"""Real CLI smoke test: ACLs, human input, observer and persisted restart.

Run after cargo build: python3 examples/agent-demo/test_demo.py (non-root).
"""
import importlib.util
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time
import unittest

SPEC = importlib.util.spec_from_file_location('demo', Path(__file__).with_name('demo.py'))
demo = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(demo)


class DemoTest(unittest.TestCase):
    def test_roundtrip_permissions_and_restart(self):
        with tempfile.TemporaryDirectory(prefix='mynofy-demo-') as temp:
            demo.STATE = Path(temp) / 'state'
            demo.setup()
            self.assertIn(str(demo.STATE), (demo.STATE / 'AGENT-COMMANDS.md').read_text())
            self.assertNotIn('@MCP_', (demo.STATE / 'AGENT-COMMANDS.md').read_text())
            env = dict(os.environ, MYNOFY_DEMO_DIR=str(demo.STATE))
            server = None
            observer = None
            try:
                def start():
                    proc = subprocess.Popen([str(demo.BIN), 'serve', '--config',
                                             str(demo.STATE / 'config.yaml')],
                                            stdout=subprocess.DEVNULL,
                                            stderr=subprocess.DEVNULL)
                    for _ in range(100):
                        if proc.poll() is not None:
                            self.fail('Service exited during startup')
                        try:
                            result = subprocess.run([str(demo.BIN), 'admin', 'status', '--socket',
                                                     str(demo.STATE / 'run/admin.sock')],
                                                    capture_output=True, timeout=2)
                            if result.returncode == 0:
                                return proc
                        except subprocess.TimeoutExpired:
                            pass
                        time.sleep(0.05)
                    proc.terminate(); proc.wait()
                    self.fail('Service did not become ready')

                server = start()
                for role in demo.ROLES:
                    client = demo.Client(role)
                    try:
                        for topic in demo.TOPICS:
                            self.assertEqual(client.request('poll', topic=topic)['type'], 'messages')
                            allowed = (role == 'human' or
                                       role == 'operator' and topic != demo.TOPICS[0] or
                                       role.startswith('worker-') and topic == demo.TOPICS[2])
                            if allowed:
                                client.request('publish', topic=topic, payload={'text': role})
                            else:
                                with self.assertRaises(RuntimeError):
                                    client.request('publish', topic=topic, payload={'text': 'denied'})
                    finally:
                        client.close()
                writer = demo.Client('worker-1')
                try:
                    catalog = writer.request('get_catalog', name='mynofy', catalog_version=1)['catalog']
                    def identifier(group, name):
                        return next(value['id'] for value in catalog[group] if value['name'] == name)
                    event = dict(kind='event', catalog=catalog['catalog_id'],
                                 catalog_version=catalog['version'],
                                 category=identifier('categories', 'JOB'),
                                 action=identifier('actions', 'START'),
                                 status=identifier('statuses', 'SUCCESS'),
                                 parameters=[{'u32': 1}])
                    writer.request('publish', topic=demo.TOPICS[2], payload=event)
                finally:
                    writer.close()
                subprocess.run(['python3', str(Path(demo.__file__)), 'human'],
                               input='Hallo "Operator" – äöü\n', text=True,
                               env=env, check=True, capture_output=True, timeout=10)
                observer = subprocess.Popen(['python3', str(Path(demo.__file__)), 'watch'],
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
                output = b''
                with selectors.DefaultSelector() as selector:
                    selector.register(observer.stdout, selectors.EVENT_READ)
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        if selector.select(0.2):
                            output += os.read(observer.stdout.fileno(), 65536)
                        if (all(('[' + topic + ']').encode() in output for topic in demo.TOPICS)
                                and b'Operator' in output and b'"kind": "event"' in output):
                            break
                self.assertIn('Hallo "Operator" – äöü', output.decode())
                self.assertIn('"kind": "event"', output.decode())
                for topic in demo.TOPICS:
                    self.assertIn('[' + topic + ']', output.decode())
                observer.terminate(); observer.wait(timeout=5)
                observer.stdout.close(); observer.stderr.close(); observer = None
                subprocess.run([str(demo.BIN), 'admin', 'content', 'save', '--socket',
                                str(demo.STATE / 'run/admin.sock')], check=True, capture_output=True)
                for _ in range(100):
                    response = json.loads(subprocess.check_output(
                        [str(demo.BIN), 'admin', 'status', '--socket', str(demo.STATE / 'run/admin.sock')]))
                    storage = response['content_storage']
                    if not storage['dirty'] and not storage['save_in_progress']:
                        self.assertIsNone(storage['last_error'])
                        break
                    time.sleep(0.05)
                else:
                    self.fail('Save did not complete')
                server.terminate(); server.wait(timeout=10)
                server = start()
                client = demo.Client('observer')
                try:
                    messages = client.request('poll', topic=demo.TOPICS[0])['items']
                    self.assertTrue(any(item['message']['payload'].get('text') ==
                                        'Hallo "Operator" – äöü' for item in messages))
                    events = client.request('poll', topic=demo.TOPICS[2])['items']
                    self.assertTrue(any(item['message']['payload'] == event for item in events))
                finally:
                    client.close()
            finally:
                for proc in [observer, server]:
                    if proc is not None:
                        if proc.poll() is None:
                            proc.terminate(); proc.wait(timeout=10)
                        if proc is observer:
                            proc.stdout.close(); proc.stderr.close()


if __name__ == '__main__':
    unittest.main()
