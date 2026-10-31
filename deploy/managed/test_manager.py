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


class ModelsTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / 'fake.py').write_text(FAKE)
        (self.root / 'pack.basalt').write_bytes(b'synthetic test fixture')
        self.catalog = self.root / 'models.json'
        self.doc = {'default':'alpha','models':[
            {'id':key,'path':str(self.root/'pack.basalt'),'parallel':2,'context_tokens':262144,'slot_full_context':True}
            for key in ('alpha','beta','unsafe')]}
        self.catalog.write_text(json.dumps(self.doc))
        _, models = catalog_from_file(self.catalog)
        self.manager = Manager(models,self.root,program=[sys.executable,'-B',str(self.root/'fake.py')],timeout=0.8)

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

    def test_backend_without_privacy_is_stopped_and_not_reported_as_loaded(self):
        self.assertEqual(self.manager.command({'action':'load','model':'unsafe'})[0],503)
        self.assertIsNone(self.manager.process)
        self.assertIsNone(self.manager.snapshot()['active_model'])
        self.assertFalse(self.manager.socket.exists())

    def test_catalog_rejects_unknown_fields_and_invalid_profiles(self):
        for field,value in [('parallel',9),('context_tokens',True),('slot_full_context','yes'),('before_load','curl example.org')]:
            with self.subTest(field=field):
                doc=json.loads(json.dumps(self.doc));doc['models'][0][field]=value
                self.catalog.write_text(json.dumps(doc))
                with self.assertRaises(ValueError):catalog_from_file(self.catalog)
        self.doc['models'][1]['id']='alpha';self.catalog.write_text(json.dumps(self.doc))
        with self.assertRaises(ValueError):catalog_from_file(self.catalog)


if __name__ == '__main__':
    unittest.main()
