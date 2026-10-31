"""Explicit real-GPU test: two local packs, one encrypted tunnel, numeric results only."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import tomllib


def run(args):
    return subprocess.run(args,check=True,capture_output=True,text=True).stdout.strip()


def call(path, token, method, route, body=None, stream=False):
    connection=http.client.HTTPConnection('localhost',timeout=600)
    connection.sock=socket.socket(socket.AF_UNIX);connection.sock.settimeout(600);connection.sock.connect(str(path))
    headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'}
    started=time.monotonic()
    try:
        connection.request(method,route,json.dumps(body) if body is not None else None,headers)
        response=connection.getresponse()
        if not stream:
            raw=response.read(65537)
            if len(raw)>65536:raise RuntimeError('oversized control response')
            return response.status,json.loads(raw),time.monotonic()-started
        first=None;usage=None;done=False;reasoning=False;content=False;error_events=0
        for line in response:
            if not line.startswith(b'data:'):continue
            event=line[5:].strip()
            if event==b'[DONE]':done=True;break
            data=json.loads(event)
            if 'error' in data:error_events+=1
            if isinstance(data.get('usage'),dict):
                usage={key:data['usage'].get(key) for key in ('prompt_tokens','completion_tokens','total_tokens')}
            for choice in data.get('choices',[]):
                delta=choice.get('delta',{})
                reasoning=reasoning or bool(delta.get('reasoning_content') or delta.get('reasoning'))
                content=content or bool(delta.get('content'))
                if first is None and (reasoning or content):first=time.monotonic()-started
        return {'status':response.status,'done':done,'reasoning_received':reasoning,'content_received':content,
            'first_token_seconds':first,'elapsed_seconds':time.monotonic()-started,'usage':usage,'error_events':error_events}
    finally:
        connection.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coder',type=Path,required=True);parser.add_argument('--nvfp4',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--backend-image',default='k-lani-link-basalt:local')
    parser.add_argument('--link-image',default='k-lani-link-host:local')
    parser.add_argument('--memory',default='6g');parser.add_argument('--tokens',type=int,default=128)
    args=parser.parse_args();os.umask(0o077)
    coder=args.coder.resolve(strict=True);nvfp4=args.nvfp4.resolve(strict=True)
    if not coder.is_file() or not nvfp4.is_file() or coder.stat().st_dev!=nvfp4.stat().st_dev:
        raise RuntimeError('both model files must be on the same filesystem')
    if len(Path('/proc/swaps').read_text().splitlines())!=1:raise RuntimeError('host swap must be disabled')
    root=args.output.resolve();root.mkdir(mode=0o700)
    runtime=Path(tempfile.mkdtemp(prefix='k-lani-link-managed-',dir='/dev/shm'))
    for folder in ('backend','api'):(runtime/folder).mkdir(mode=0o700)
    for folder in ('host-state','client-state'):(root/folder).mkdir(mode=0o700)
    listener=socket.socket();listener.bind(('127.0.0.1',0));port=listener.getsockname()[1];listener.close()
    run(['docker','run','--rm','--network','none','--user',f'{os.getuid()}:{os.getgid()}',
        '-v',f'{root}:/setup',args.link_image,'init','--dir','/setup/pair','--signaling-url',f'http://127.0.0.1:{port}'])
    token=secrets.token_urlsafe(32);(root/'management-token').write_text(token)
    for role in ('host','client'):
        p=root/'pair'/f'{role}.toml';text=p.read_text()
        text=text.replace('max_concurrent_requests = 1','max_concurrent_requests = 2').replace('max_queued_requests = 0','max_queued_requests = 2')
        extra='include_loopback_candidate = true\npairing_state_dir = "/var/lib/link"\n'
        if role=='host':
            extra+='model_control_socket = "/run/link/control.sock"\nupstream_socket = "/run/link/basalt.sock"\n'
        else:extra+='management_token_file = "/run/secrets/management-token"\n'
        p.write_text(extra+text)
    catalog={'default':None,'models':[{'id':key,'path':f'/models/{key}.basalt','context_tokens':262144,
        'parallel':2,'slot_full_context':True,'prefill':512,'reasoning_budget_tokens':2048} for key in ('coder','nvfp4')]}
    (root/'models.json').write_text(json.dumps(catalog,indent=2)+'\n')
    prefix='link-managed-'+secrets.token_hex(4);containers=[];records=[];samples=[];stop=threading.Event()
    report={'profile':{'context_tokens_per_slot':262144,'parallel':2,'kv':'int8','prefill':512,
        'backend_memory_limit':args.memory,'swap_disabled':True,'both_peers_on_one_host':True,
        'filled_context_test':False,'model_files_same_filesystem':True},'runs':records,'samples':samples}
    def start(role,options,image,command):
        name=prefix+'-'+role
        run(['docker','run','-d','--name',name,'--read-only','--cap-drop','ALL','--security-opt','no-new-privileges',
            '--log-driver','none','--ulimit','core=0',*options,image,*command]);containers.append(name);return name
    def sample():
        last_resource=0
        while not stop.wait(0.1):
            sample={'time':time.monotonic(),'swap_disabled':len(Path('/proc/swaps').read_text().splitlines())==1}
            try:
                data=json.loads((runtime/'backend/performance.json').read_text())
                if time.time()-data['observed_at']<=5:sample['performance']=data
            except (OSError,ValueError,KeyError):pass
            if time.monotonic()-last_resource>=2:
                try:
                    sample['containers']=[{key:row.get(key) for key in ('Name','CPUPerc','MemUsage','BlockIO','NetIO','PIDs')}
                        for row in map(json.loads,run(['docker','stats','--no-stream','--format','{{json .}}',*containers]).splitlines())]
                    sample['gpu']=run(['nvidia-smi','--query-gpu=utilization.gpu,memory.used,power.draw,power.limit','--format=csv,noheader,nounits'])
                except subprocess.CalledProcessError:pass
                last_resource=time.monotonic()
            samples.append(sample)
    sampler=None
    try:
        backend=start('basalt',['--network','none','--gpus','all','--memory',args.memory,'--memory-swap',args.memory,
            '-e','LINK_REQUIRE_NO_SWAP=1','-e','LINK_NUMERIC_METRICS=1',
            '--tmpfs','/tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777',
            '--tmpfs','/home/link/.cache:rw,nosuid,nodev,noexec,size=256m,uid=1000,gid=1000,mode=0700',
            '-v',f'{coder}:/models/coder.basalt:ro','-v',f'{nvfp4}:/models/nvfp4.basalt:ro',
            '-v',f'{root}/models.json:/run/secrets/models.json:ro','-v',f'{runtime}/backend:/run/link',
            '--entrypoint','/opt/basalt/.venv/bin/python'],args.backend_image,['-B','/opt/link-manager.py'])
        signal=start('signal',['--network','host','--memory','128m','--memory-swap','128m',
            '-v',f'{root}/pair/signaling-token.txt:/run/secrets/token:ro'],args.link_image,
            ['signal','--listen',f'127.0.0.1:{port}','--token-file','/run/secrets/token'])
        for role in ('host','client'):
            options=['--network','host','--memory','256m','--memory-swap','256m',
                '-v',f'{root}/pair/{role}.toml:/run/secrets/config.toml:ro',
                '-v',f'{root}/{role}-state:/var/lib/link']
            command=[role,'--config','/run/secrets/config.toml']
            if role=='host':options+=['-v',f'{runtime}/backend:/run/link:ro']
            else:
                options+=['-v',f'{runtime}/api:/run/link-api','-v',f'{root}/management-token:/run/secrets/management-token:ro']
                command+=['--api-socket','/run/link-api/api.sock']
            start(role,options,args.link_image,command)
        api=runtime/'api/api.sock';key=tomllib.loads((root/'pair/client.toml').read_text())['api_key']
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            try:
                if call(api,key,'GET','/health')[1].get('connected'):break
            except (OSError,ValueError,http.client.HTTPException):pass
            time.sleep(0.1)
        else:raise RuntimeError('tunnel did not become ready')
        run(['docker','stop','--timeout','2',signal]);report['signaling_stopped']=True
        markers=sorted(str(p.relative_to(root)) for p in root.glob('*-state/*.used'))
        if len(markers)!=2:raise RuntimeError('pair consumption was not persisted on both peers')
        sampler=threading.Thread(target=sample,daemon=True);sampler.start()
        for model in ('coder','nvfp4','coder'):
            status,state,elapsed=call(api,token,'POST','/v1/link/model',{'action':'load','model':model})
            record={'model':model,'load_status':status,'load_seconds':elapsed};records.append(record)
            print(json.dumps(record),flush=True)
            if status!=200:
                record['error']=state;raise RuntimeError('model load failed')
            record['verified_profile']=state
            record['pairs']=[]
            for iteration in range(2):
                begin=time.monotonic()
                def infer(index):
                    body={'model':model,'messages':[{'role':'user','content':'List the integers from 1 to 150 in order. Separate them with commas. Do not explain.'}],
                        'stream':True,'stream_options':{'include_usage':True},'max_tokens':args.tokens,'reasoning_budget_tokens':0,'temperature':0}
                    return call(api,key,'POST','/v1/chat/completions',body,True)
                with ThreadPoolExecutor(max_workers=2) as pool:results=list(pool.map(infer,range(2)))
                active=max((s.get('performance',{}).get('active_requests',0) for s in samples if s['time']>=begin),default=0)
                pair={'iteration':iteration+1,'requests':results,'max_observed_active':active};record['pairs'].append(pair)
                print(json.dumps({'model':model,**pair}),flush=True)
                if any(r['status']!=200 or not r['done'] or not r['usage'] or r['error_events'] or not (r['content_received'] or r['reasoning_received']) for r in results):
                    raise RuntimeError('inference stream failed')
                if active<2:raise RuntimeError('two simultaneous backend requests were not observed')
            if not call(api,key,'GET','/health')[1].get('connected'):raise RuntimeError('tunnel was lost')
            if sorted(str(p.relative_to(root)) for p in root.glob('*-state/*.used'))!=markers:raise RuntimeError('pairing changed')
        if call(api,token,'POST','/v1/link/model',{'action':'unload'})[0]!=200:raise RuntimeError('unload failed')
        report['same_tunnel_survived_all_changes']=call(api,key,'GET','/health')[1].get('connected')
        report['memory_events']=run(['docker','exec',backend,'cat','/sys/fs/cgroup/memory.events'])
        if any(line.startswith('oom_kill ') and int(line.split()[1])>0 for line in report['memory_events'].splitlines()):raise RuntimeError('backend OOM occurred')
        if any(not sample.get('swap_disabled',False) for sample in samples):raise RuntimeError('host swap was enabled during the test')
        report['success']=True
    except Exception as error:
        report['success']=False;report['failure_type']=type(error).__name__
        raise
    finally:
        stop.set()
        if sampler:sampler.join(timeout=5)
        if containers:
            try:
                report['container_isolation']=[{'name':row['Name'], 'network_mode':row['HostConfig']['NetworkMode'],
                    'read_only':row['HostConfig']['ReadonlyRootfs'], 'log_driver':row['HostConfig']['LogConfig']['Type'],
                    'memory':row['HostConfig']['Memory'], 'memory_swap':row['HostConfig']['MemorySwap']}
                    for row in json.loads(run(['docker','inspect',*containers]))]
                report['memory_events']=run(['docker','exec',containers[0],'cat','/sys/fs/cgroup/memory.events'])
            except subprocess.CalledProcessError: pass
        (root/'results.json').write_text(json.dumps(report,indent=2)+'\n')
        for name in reversed(containers):
            subprocess.run(['docker','stop','--timeout','12',name],capture_output=True)
            subprocess.run(['docker','rm',name],capture_output=True)
        shutil.rmtree(runtime,ignore_errors=True)
    print('PASS: model changes and concurrent streams used one tunnel; signal stayed stopped.',flush=True)


if __name__=='__main__':main()
