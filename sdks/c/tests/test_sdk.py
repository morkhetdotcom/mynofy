"""Run a separately compiled C consumer against temporary Rust services."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[3]
# Share the stdlib-only real-service fixture; no Bash commands or other SDK runtime.
spec = importlib.util.spec_from_file_location('mynofy_test_fixture', ROOT / 'sdks/bash/tests/test_sdk.py')
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
NATIVE = os.environ.get('MYNOFY_C_TEST', str(ROOT / 'target/c-sdk-tests/native'))


class Native(unittest.TestCase):
    def fixture(self, mode):
        fixture = fixtures.Fixture(mode)
        self.addCleanup(fixture.close)
        return fixture

    def run_native(self, fixture, mode, operation, key='client.pk8'):
        address = str(fixture.socket) if mode == 'unix' else fixture.env['MYNOFY_URL']
        ca = '' if mode == 'unix' else str(fixture.dir / 'ca.pem')
        fresh = fixture.dir / 'fresh.pk8'
        command = [NATIVE, operation, mode, address, ca, str(fixture.dir / key), str(fresh)]
        p = subprocess.run(command, capture_output=True, text=True, timeout=25)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn('native C checks passed', p.stdout)
        return fresh

    def test_unix(self):
        self.run_native(self.fixture('unix'), 'unix', 'roundtrip')

    def test_wss(self):
        self.run_native(self.fixture('wss'), 'wss', 'roundtrip')

    def test_acl(self):
        self.run_native(self.fixture('unix'), 'unix', 'denied', 'other.pk8')

    def test_rotation_and_cli_interop(self):
        f = self.fixture('unix')
        fresh = self.run_native(f, 'unix', 'rotate')
        self.assertEqual(fresh.stat().st_mode & 0o777, 0o600)
        p = subprocess.run([str(fixtures.BIN), 'client', '--socket', str(f.socket), '--owner', 'local',
                            '--key', str(fresh), '--client-id', f.key['client_id']],
                           input=json.dumps(dict(version=1, type='get_catalog', id='cli', name='mynofy', catalog_version=1))+'\n',
                           capture_output=True, text=True, timeout=15)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(json.loads(p.stdout)['type'], 'catalog')

    def test_standalone_example(self):
        f = self.fixture('unix')
        example = os.environ.get('MYNOFY_C_EXAMPLE', str(ROOT / 'target/c-sdk-tests/local'))
        p = subprocess.run([example, str(f.socket), str(f.dir/'client.pk8'), 'local', 'local/events'],
                           capture_output=True, text=True, timeout=15)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn('Published:', p.stdout)
        self.assertIn('Next cursor:', p.stdout)

    def test_tls_failure(self):
        f = self.fixture('wss')
        f.env['MYNOFY_URL'] = f.env['MYNOFY_URL'].replace('localhost', '127.0.0.1')
        p = subprocess.run([NATIVE, 'tls-reject', 'wss', f.env['MYNOFY_URL'], str(f.dir/'ca.pem'),
                            str(f.dir/'client.pk8'), str(f.dir/'fresh.pk8')], capture_output=True, timeout=10)
        self.assertEqual(p.returncode, 0, p.stderr)


if __name__ == '__main__':
    unittest.main()
