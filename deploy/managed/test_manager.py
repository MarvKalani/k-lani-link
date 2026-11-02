"""Real child-process reloads with a synthetic offline backend; no model or GPU needed."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from manager import Manager, catalog_from_file, health

FAKE = r'''
import argparse, json, socketserver
from http.server import BaseHTTPRequestHandler
p=argparse.ArgumentParser();p.add_argument('--unix-socket');p.add_argument('--alias');p.add_argument('--parallel',type=int);p.add_argument('--max-context',type=int);p.add_argument('--slot-full-context',action='store_true');args,_=p.parse_known_args()
class H(BaseHTTPRequestHandler):
 def log_message(self,*a): pass
 def do_GET(self):
  privacy={'version':1,'application_logs':False,'request_history':False,'diagnostics':False,'remote_images':False}
  if args.alias=='unsafe': privacy['application_logs']=True
  data=json.dumps({'loaded':True,'model':args.alias,'parallel':args.parallel,'max_context':args.max_context if args.slot_full_context else args.max_context//args.parallel,'link_privacy':privacy}).encode()
  self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
class S(socketserver.UnixStreamServer): pass
S(args.unix_socket,H).serve_forever()
'''


FAKE_LLAMA = r'''
import argparse, json, os, socketserver, sys
from http.server import BaseHTTPRequestHandler
p=argparse.ArgumentParser();p.add_argument('--host');p.add_argument('--alias');p.add_argument('--parallel',type=int);p.add_argument('--ctx-size',type=int);args,rest=p.parse_known_args()
# The supervisor must switch off everything that could keep or show a request.
if not {'--no-webui','--no-slots','--log-disable','--offline','--kv-unified'} <= set(rest) or '/lib:' not in os.environ.get('LD_LIBRARY_PATH',''): sys.exit(3)
class H(BaseHTTPRequestHandler):
 def log_message(self,*a): pass
 def do_GET(self):
  body={'/health':{'status':'ok'},'/props':{'total_slots':args.parallel,'default_generation_settings':{'n_ctx':args.ctx_size}},'/v1/models':{'data':[{'id':args.alias}]}}.get(self.path)
  data=json.dumps(body).encode();self.send_response(200 if body else 404);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
class S(socketserver.UnixStreamServer): pass
S(args.host,H).serve_forever()
'''


class ModelsTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / 'fake.py').write_text(FAKE)
        (self.root / 'pack.basalt').write_bytes(b'synthetic test fixture')
        self.catalog = self.root / 'models.json'
        self.doc = {'default':'alpha','models':[
            {'id':key,'path':str(self.root/'pack.basalt'),'parallel':2,'context_tokens':262144,'slot_full_context':True}
            for key in ('alpha','beta','unsafe')]+[
            {'id':'Mellum2.1-Q8_0','engine':'llama','path':str(self.root/'model.gguf'),'parallel':2,'context_tokens':32768,'kv':'q4_0'}]}
        (self.root / 'model.gguf').write_bytes(b'synthetic test fixture')
        (self.root / 'lib').mkdir()
        self.catalog.write_text(json.dumps(self.doc))
        _, models = catalog_from_file(self.catalog)
        (self.root / 'fake_llama.py').write_text(FAKE_LLAMA)
        self.manager = Manager(models,self.root,program=[sys.executable,'-B',str(self.root/'fake.py')],timeout=0.8,
            llama=[str(self.root/'llama-server'),'-B',str(self.root/'fake_llama.py')])
        (self.root / 'llama-server').symlink_to(sys.executable)

    def tearDown(self):
        self.manager.stop_engine()
        self.directory.cleanup()

    def test_load_switch_unload_without_arbitrary_paths_and_without_output(self):
        output=io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            code,state=self.manager.command({'action':'load','model':'alpha'})
            self.assertEqual(code,200);self.assertEqual(state['active_model'],'alpha')
            first=self.manager.process
            self.assertEqual(self.manager.command({'action':'load','model':'../../secret'})[0],400)
            self.assertEqual(self.manager.command({'action':'load','model':'missing'})[0],404)
            self.assertEqual(self.manager.command({'action':'load','model':'beta','path':'/etc/passwd'})[0],400)
            self.assertIs(self.manager.process,first)
            self.assertEqual(self.manager.command({'action':'load','model':'beta'})[0],200)
            self.assertIsNotNone(first.poll());self.assertNotEqual(self.manager.process.pid,first.pid)
            self.assertEqual(health(self.manager.socket)['model'],'beta')
            self.assertEqual(self.manager.command({'action':'unload'})[0],200)
        self.assertEqual(output.getvalue(),'')
        self.assertFalse(self.manager.socket.exists())
        self.assertIsNone(self.manager.snapshot()['active_model'])
        self.assertNotIn('path',json.dumps(self.manager.snapshot()))

    def test_a_gguf_model_runs_under_llama_cpp_and_is_vouched_for_by_the_supervisor(self):
        output=io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            self.assertEqual(self.manager.command({'action':'load','model':'alpha'})[0],200)
            code,state=self.manager.command({'action':'load','model':'Mellum2.1-Q8_0'})
            self.assertEqual(code,200,state);self.assertEqual(state['active_model'],'Mellum2.1-Q8_0')
            rows={row['id']:row for row in state['models']}
            self.assertEqual(rows['Mellum2.1-Q8_0']['privacy'],'supervised');self.assertEqual(rows['Mellum2.1-Q8_0']['engine'],'llama')
            self.assertIsNone(rows['alpha']['privacy'])
            self.assertEqual(self.manager.command({'action':'load','model':'beta'})[0],200)
            self.assertEqual(self.manager.snapshot()['models'][1]['privacy'],'basalt')
        self.assertEqual(output.getvalue(),'')

    def test_backend_without_privacy_is_stopped_and_not_reported_as_loaded(self):
        self.assertEqual(self.manager.command({'action':'load','model':'unsafe'})[0],503)
        self.assertIsNone(self.manager.process)
        self.assertIsNone(self.manager.snapshot()['active_model'])
        self.assertFalse(self.manager.socket.exists())

    def test_catalog_rejects_unknown_fields_and_invalid_profiles(self):
        for field,value in [('parallel',9),('context_tokens',True),('slot_full_context','yes'),('before_load','curl example.org'),
                ('engine','vllm'),('kv','q4_0'),('id','../x'),('id','.hidden')]:
            with self.subTest(field=field):
                doc=json.loads(json.dumps(self.doc));doc['models'][0][field]=value
                self.catalog.write_text(json.dumps(doc))
                with self.assertRaises(ValueError):catalog_from_file(self.catalog)
        self.doc['models'][1]['id']='alpha';self.catalog.write_text(json.dumps(self.doc))
        with self.assertRaises(ValueError):catalog_from_file(self.catalog)


if __name__ == '__main__':
    unittest.main()
