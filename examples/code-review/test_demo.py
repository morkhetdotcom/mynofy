"""Exercise the documented English example against a real service, without LLMs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path(__file__).with_name('demo.py')
BIN = Path(os.environ.get('MYNOFY_BIN', ROOT / 'target/debug/mynofy')).resolve()


class Example(unittest.TestCase):
    def test_setup_exchange_permissions_and_english_prompts(self):
        if os.geteuid() == 0:
            self.skipTest('Local service example requires an unprivileged account')
        with tempfile.TemporaryDirectory(prefix='mn-review-') as directory:
            state = Path(directory) / 'state'
            env = dict(os.environ, MYNOFY_REVIEW_DIR=str(state), MYNOFY_BIN=str(BIN))
            def run(*args, check=True):
                result = subprocess.run([sys.executable, str(SCRIPT), *args], env=env,
                                        capture_output=True, text=True, timeout=30)
                if check:
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return result
            run('setup')
            prompts = (state / 'AGENT-COMMANDS.md').read_text()
            self.assertNotIn('@MCP_', prompts)
            self.assertNotIn('@STATE@', prompts)
            self.assertIn('Codex prompt', prompts)
            self.assertIn('Claude prompt', prompts)
            self.assertIn('review/human', prompts)
            self.assertNotEqual(run('setup', check=False).returncode, 0)
            server = subprocess.Popen([str(BIN), 'serve', '--config', str(state / 'config.yaml')],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            try:
                for _ in range(100):
                    self.assertIsNone(server.poll(), 'Service exited during startup')
                    status = subprocess.run([str(BIN), 'admin', 'status', '--socket',
                                             str(state / 'run/admin.sock')], capture_output=True, timeout=2)
                    if status.returncode == 0:
                        break
                    time.sleep(.05)
                else:
                    self.fail('Service was not ready within startup window')
                output = run('exchange').stdout
                self.assertIn('JOB/COMPLETE/SUCCESS', output)
                self.assertIn('worker human-topic publish was denied', output)
                self.assertIn('owner-stamped producer remains Claude', output)
                self.assertIn('Observer ignored the actor mismatch', output)
                self.assertIn('no code was generated', output)
            finally:
                server.terminate()
                try:
                    server.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.communicate(timeout=5)

    def test_event_names_are_available_in_builtin_catalog(self):
        catalog = json.loads((ROOT / 'protocol/catalogs/mynofy-v1.json').read_text())
        expected = {'categories': {'JOB': 12}, 'actions': {'START': 10, 'ACQUIRE': 72, 'COMPLETE': 42},
                    'statuses': {'SUCCESS': 2, 'FAILED': 3, 'PENDING': 7}}
        for namespace, names in expected.items():
            actual = {x['name']: x['id'] for x in catalog[namespace]}
            for name, value in names.items():
                self.assertEqual(actual[name], value)


if __name__ == '__main__':
    unittest.main()
