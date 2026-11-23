"""Bash API tests; only Python's standard library, the Rust CLI, jq and OpenSSL."""
import base64
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[3]
BIN = Path(os.environ.get('MYNOFY_BIN', ROOT / 'target/debug/mynofy'))
SDK = ROOT / 'sdks/bash/mynofy.sh'
BASH = os.environ.get('MYNOFY_BASH', 'bash')
TOPIC = 'local/events'


def environment(**extra):
    env = {k: v for k, v in os.environ.items() if not k.startswith('MYNOFY_') and k != 'BASH_ENV'}
    env.update(MYNOFY_BIN=str(BIN), MYNOFY_BASH_SDK=str(SDK))
    env.update(extra)
    return env


def call(body, *args, env=None):
    return subprocess.run([BASH, '-euo', 'pipefail', '-c', 'source "$MYNOFY_BASH_SDK"\n' + body, '--', *args],
                          env=env or environment(), capture_output=True, text=True, timeout=20)


class Builders(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='mn-bash-unit-')
        self.addCleanup(self.temp.cleanup)
        self.dir = Path(self.temp.name)
        fake = self.dir / 'fake cli.py'
        fake.write_text(f'#!{sys.executable}\n' + '''import json,os,sys,time,signal
from pathlib import Path
request=sys.stdin.read()
if request.strip(): value=json.loads(request)
else: value={}
Path(os.environ['CAPTURE']).write_text(json.dumps({'request':value,'args':sys.argv[1:]}))
mode=os.environ.get('FAKE_MODE','normal')
if mode=='wait':
 Path(os.environ['CAPTURE']+'.pid').write_text(str(os.getpid()))
 signal.signal(signal.SIGTERM,lambda *_:sys.exit(0))
 time.sleep(15)
 sys.exit(0)
if mode=='failure':sys.exit(1)
kind={'publish':'result','poll':'messages','list_messages':'messages_page','get_message':'message_status','get_permissions':'permissions','get_catalog':'catalog'}.get(value.get('type'),'result')
reply={'version':1,'type':kind,'request_id':value.get('id'),'opaque_u64':18446744073709551615}
if mode=='wrong-id':reply['request_id']='wrong'
if mode=='wrong-kind':reply['type']='auth_welcome'
if mode=='wrong-version':reply['version']=2
if mode.startswith('broker'):
 reply.update(type='error',code='rate_limited',detail='wait',retry_after_ms=100)
 if mode!='broker':reply.pop('request_id')
 if mode=='broker-no-backoff':reply.pop('retry_after_ms')
 if mode=='broker-fraction':reply['retry_after_ms']=1.5
 if mode=='broker-negative':reply['retry_after_ms']=-1
 if mode=='broker-other':reply['code']='unauthorized'
 if mode=='broker-wrong-id':reply['request_id']='other'
if mode=='malformed':print('{');sys.exit(0)
if mode=='empty':sys.exit(0)
print(json.dumps(reply))
if mode=='multiple':print(json.dumps(reply))
''')
        fake.chmod(0o700)
        self.capture = self.dir / 'captured.json'
        self.env = environment(MYNOFY_BIN=str(fake), MYNOFY_OWNER='local', MYNOFY_KEY='key with spaces.pk8',
                               MYNOFY_SOCKET='socket with spaces', CAPTURE=str(self.capture), TMPDIR=str(self.dir))

    def run_ok(self, body, *args):
        p = call(body, *args, env=self.env)
        self.assertEqual(p.returncode, 0, p.stderr)
        return json.loads(p.stdout)

    def test_shared_publish_vectors_and_u64_output(self):
        frames = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())['frames']
        for vector in frames:
            v = vector['json']
            if v['type'] != 'publish':
                continue
            payload = v['payload']
            if payload['kind'] == 'bytes':
                data = self.dir / 'bytes'
                data.write_bytes(base64.b64decode(payload['data']))
                reply = self.run_ok('mynofy_publish_file "$1" "$2" "$3"', v['topic'], str(data), payload['content_type'])
            else:
                parameters = [f'{kind}:{str(value).lower()}' for parameter in payload['parameters'] for kind, value in parameter.items()]
                reply = self.run_ok('mynofy_publish_event "$@"', v['topic'], str(payload['category']), str(payload['action']), str(payload['status']), *parameters)
            captured = json.loads(self.capture.read_text())['request']
            expected = dict(v, id='bash-sdk')
            self.assertEqual(captured, expected)
            self.assertEqual(reply['opaque_u64'], 2**64 - 1)

    def test_shared_other_request_vectors(self):
        frames = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())['frames']
        for vector in frames:
            value = vector['json']
            kind = value['type']
            command = None
            if kind == 'poll':
                command = ('mynofy_poll "$@"', value['topic'], value.get('after', ''), str(value.get('limit', 20)))
            elif kind == 'list_messages':
                command = ('mynofy_list_messages "$@"', value.get('topic', ''), value.get('cursor', ''), str(value.get('limit', 20)))
            elif kind == 'get_message':
                command = ('mynofy_get_message "$@"', value['message_id'])
            elif kind == 'get_permissions':
                command = ('mynofy_get_permissions "$@"', value['topic'])
            elif kind == 'get_catalog':
                command = ('mynofy_get_catalog "$@"', value['name'], str(value['catalog_version']))
            if command:
                with self.subTest(kind=kind):
                    self.run_ok(*command)
                    self.assertEqual(json.loads(self.capture.read_text())['request'], dict(value, id='bash-sdk'))

    def test_peer_options_and_conflicting_transport(self):
        self.env.update(MYNOFY_VIA_TOPIC='remote/work', MYNOFY_OWNER_CA='owner ca.pem', MYNOFY_OWNER_NAME='owner')
        self.run_ok('mynofy_get_catalog')
        args = json.loads(self.capture.read_text())['args']
        self.assertEqual(args[-6:], ['--via-topic', 'remote/work', '--owner-ca', 'owner ca.pem', '--owner-name', 'owner'])
        self.capture.unlink()
        self.env['MYNOFY_URL'] = 'wss://localhost:8090'
        self.assertEqual(call('mynofy_get_catalog', env=self.env).returncode, 64)
        self.assertFalse(self.capture.exists())

    def test_quoting_argv_and_optional_fields(self):
        text = 'quotes " \\ newline\n$(touch SHOULD_NOT_EXIST) `x` ä'
        self.run_ok('mynofy_publish_text "$1" "$2" "$3"', TOPIC, text, '2030-01-01T00:00:00Z')
        captured = json.loads(self.capture.read_text())
        request = captured['request']
        self.assertEqual(base64.b64decode(request['payload']['data']).decode(), text)
        self.assertEqual(request['expires']['time'], '2030-01-01T00:00:00Z')
        self.assertIn('key with spaces.pk8', captured['args'])
        self.assertIn('socket with spaces', captured['args'])
        self.assertFalse((ROOT / 'SHOULD_NOT_EXIST').exists())
        self.run_ok('mynofy_list_messages "" "signed+/cursor=" 1')
        request = json.loads(self.capture.read_text())['request']
        self.assertNotIn('topic', request)
        self.assertEqual(request['cursor'], 'signed+/cursor=')
        self.run_ok('mynofy_publish_event local/events 12 42 2 f32:1.25 f64:-2.5e3 bool:false --expires 2030-01-01T00:00:00Z')
        request = json.loads(self.capture.read_text())['request']
        self.assertEqual(request['payload']['parameters'], [{'f32': 1.25}, {'f64': -2500}, {'bool': False}])

    def test_invalid_inputs_never_start_cli(self):
        commands = [
            'mynofy_poll local/events bad', 'mynofy_poll local/events "" 0',
            'mynofy_publish_event t 12 42 2 i64:-9223372036854775809',
            'mynofy_publish_event t 12 42 2 i64:9223372036854775808',
            'mynofy_publish_event t 12 42 2 u32:4294967296',
            'mynofy_publish_event t 12 42 2 i32:-2147483649',
            'mynofy_publish_event t 12 42 2 bool:0',
            'mynofy_publish_event t 12 42 2 f64:NaN',
            'mynofy_publish_event t 12 42 2 bool:true bool:true bool:true bool:true',
        ]
        for command in commands:
            with self.subTest(command=command):
                p = call(command, env=self.env)
                self.assertEqual(p.returncode, 64, p.stderr)
                self.assertFalse(self.capture.exists())
        for size in [4097, 100_000]:
            path = self.dir / 'large';path.write_bytes(b'x' * size)
            p = call('mynofy_publish_file t "$1"', str(path), env=self.env)
            self.assertEqual(p.returncode, 64, p.stderr)
            self.assertFalse(self.capture.exists())

    def test_uncorrelated_rate_limit_is_broker_error_only_with_valid_backoff(self):
        for mode in ['broker-missing', 'broker-no-backoff', 'broker-fraction',
                     'broker-negative', 'broker-other', 'broker-wrong-id']:
            with self.subTest(mode=mode):
                self.env['FAKE_MODE'] = mode
                result = call('mynofy_poll "$1"', TOPIC, env=self.env)
                if mode == 'broker-missing':
                    self.assertEqual(result.returncode, 10, result.stderr)
                    self.assertEqual(json.loads(result.stdout)['retry_after_ms'], 100)
                else:
                    self.assertEqual(result.returncode, 21, result.stderr)
                    self.assertEqual(result.stdout, '')

    def test_error_statuses_and_cleanup(self):
        for mode, status in [('broker', 10), ('failure', 20), ('wrong-id', 21), ('wrong-kind', 21),
                             ('wrong-version', 21), ('malformed', 21), ('empty', 21), ('multiple', 21)]:
            with self.subTest(mode=mode):
                env = dict(self.env, FAKE_MODE=mode)
                p = call('mynofy_publish_text local/events once', env=env)
                self.assertEqual(p.returncode, status, p.stderr)
                self.assertEqual(list(self.dir.glob('mynofy-bash.*')), [])
                if mode == 'broker':
                    self.assertEqual(json.loads(p.stdout)['retry_after_ms'], 100)
                else:
                    self.assertEqual(p.stdout, '')

    def test_source_preserves_shell_state(self):
        script = '''set -euo pipefail
trap ':' TERM
before_options=$(set +o)
before_trap=$(trap -p TERM)
before_ifs=$IFS
before_umask=$(umask)
source "$MYNOFY_BASH_SDK"
[[ $(set +o) == "$before_options" && $(trap -p TERM) == "$before_trap" && $IFS == "$before_ifs" && $(umask) == "$before_umask" ]]
mynofy_get_catalog >/dev/null
[[ $(trap -p TERM) == "$before_trap" && $(umask) == "$before_umask" ]]
'''
        p = subprocess.run([BASH, '-c', script], env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(p.returncode, 0, p.stderr)

    def test_termination_stops_cli_and_cleans_private_files(self):
        env = dict(self.env, FAKE_MODE='wait')
        p = subprocess.Popen([BASH, '-c', 'source "$MYNOFY_BASH_SDK"; mynofy_publish_text local/events once'],
                             env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        def cleanup():
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            p.communicate(timeout=3)
        self.addCleanup(cleanup)
        pidfile = Path(str(self.capture) + '.pid')
        deadline = time.monotonic() + 5
        while not pidfile.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        self.assertTrue(pidfile.exists())
        child = int(pidfile.read_text())
        # Target the operation subshell directly, as a supervising caller would.
        # Older Bash versions retain extra shells: use the actual CLI parent's PID.
        # ps -o ppid= is available on Linux and macOS. Sending to the whole
        # process group also kills the untrapped caller shell, hiding the
        # operation handler's exit status behind Python's -SIGTERM result.
        parent = subprocess.run(['ps', '-o', 'ppid=', '-p', str(child)],
                                capture_output=True, text=True, timeout=3)
        self.assertEqual(parent.returncode, 0, parent.stderr)
        operation = int(parent.stdout.strip())
        self.assertGreater(operation, 1)
        os.kill(operation, signal.SIGTERM)
        p.communicate(timeout=5)
        self.assertEqual(p.returncode, 143)
        with self.assertRaises(ProcessLookupError):
            os.kill(child, 0)
        self.assertEqual(list(self.dir.glob('mynofy-bash.*')), [])


class Fixture:
    def __init__(self, mode):
        self.temp = tempfile.TemporaryDirectory(prefix='mn-bash-integration-')
        self.dir = Path(self.temp.name)
        self.process = None
        self.log = None
        self.socket = self.dir / 'm.sock'
        try:
            self.start(mode)
        except BaseException:
            self.close()
            raise

    def start(self, mode):
        def key(name):
            return json.loads(subprocess.check_output([str(BIN), 'keygen', '--out', str(self.dir / name)], text=True))
        self.key = key('client.pk8'); self.other = key('other.pk8')
        cfg = json.loads((ROOT / 'config/scenarios/unix.json').read_text())
        cfg['service'] = dict(id='local', unix_socket=str(self.socket))
        self.env = environment(MYNOFY_OWNER='local', MYNOFY_KEY=str(self.dir / 'client.pk8'), MYNOFY_SOCKET=str(self.socket))
        if mode == 'wss':
            subprocess.run(['openssl', 'req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256',
                            '-nodes', '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost',
                            '-keyout', str(self.dir / 'ca.key'), '-out', str(self.dir / 'ca.pem')], check=True, capture_output=True)
            subprocess.run(['openssl', 'req', '-new', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256',
                            '-nodes', '-subj', '/CN=localhost', '-keyout', str(self.dir / 's.key'),
                            '-out', str(self.dir / 'server.csr')], check=True, capture_output=True)
            (self.dir / 'extensions').write_text('basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n')
            subprocess.run(['openssl', 'x509', '-req', '-in', str(self.dir / 'server.csr'),
                            '-CA', str(self.dir / 'ca.pem'), '-CAkey', str(self.dir / 'ca.key'),
                            '-CAcreateserial', '-days', '1', '-extfile', str(self.dir / 'extensions'),
                            '-out', str(self.dir / 'server.crt')], check=True, capture_output=True)
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0)); self.port = probe.getsockname()[1]
            cfg['service'] = dict(id='local', listen=f'127.0.0.1:{self.port}',
                                  tls=dict(certificate_file=str(self.dir / 'server.crt'), private_key_file=str(self.dir / 's.key')))
            del self.env['MYNOFY_SOCKET']
            self.env.update(MYNOFY_URL=f'wss://localhost:{self.port}', MYNOFY_CA=str(self.dir / 'ca.pem'))
        cfg['clients'] = [dict(client_id=k['client_id'], keys=[dict(algorithm='ed25519', public_key=k['public_key'], valid_until='2035-01-01T00:00:00Z')]) for k in [self.key, self.other]]
        cfg['topics'][0]['acl'] = [dict(client_id=self.key['client_id'], allow=['publish', 'subscribe', 'inspect_own'])]
        cfg['limits'].update(client_burst=1000, client_requests_per_second=1000)
        config = self.dir / 'config.json';config.write_text(json.dumps(cfg))
        self.log = (self.dir / 'stderr').open('w')
        self.process = subprocess.Popen([str(BIN), 'serve', '--config', str(config)], stdout=subprocess.DEVNULL, stderr=self.log)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError((self.dir / 'stderr').read_text())
            if mode == 'unix' and self.socket.exists():
                return
            if mode == 'wss':
                try:
                    with socket.create_connection(('127.0.0.1', self.port), timeout=.05):
                        return
                except OSError:
                    pass
            time.sleep(.01)
        raise RuntimeError('service startup timeout')

    def close(self):
        if self.process:
            self.process.terminate()
            try:
                self.process.wait(3)
            except subprocess.TimeoutExpired:
                self.process.kill();self.process.wait(3)
        if self.log:
            self.log.close()
        self.temp.cleanup()


@unittest.skipIf(os.getuid() == 0, 'real CLI tests need a normal UID')
class Integration(unittest.TestCase):
    def fixture(self, mode):
        f = Fixture(mode);self.addCleanup(f.close);return f

    def ok(self, f, body, *args, env=None):
        p = call(body, *args, env=env or f.env)
        self.assertEqual(p.returncode, 0, p.stderr)
        return json.loads(p.stdout)

    def test_roundtrip_unix(self):
        self.roundtrip('unix')

    def test_roundtrip_wss(self):
        self.roundtrip('wss')

    def roundtrip(self, mode):
        f = self.fixture(mode)
        self.assertEqual(self.ok(f, 'mynofy_get_catalog')['catalog']['name'], 'mynofy')
        self.assertIn('publish', self.ok(f, 'mynofy_get_permissions local/events')['actions'])
        binary = f.dir / 'bytes'; binary.write_bytes(bytes(range(256)) * 16)
        first = self.ok(f, 'mynofy_publish_file local/events "$1" application/octet-stream', str(binary))
        second = self.ok(f, 'mynofy_publish_event local/events 12 42 2 u64:18446744073709551615 i64:-9223372036854775808 bool:false')
        page = self.ok(f, 'mynofy_poll local/events "" 1')
        self.assertTrue(page['has_more']);self.assertEqual(base64.b64decode(page['items'][0]['message']['payload']['data']), binary.read_bytes())
        next_page = self.ok(f, 'mynofy_poll local/events "$1"', page['next_cursor'])
        self.assertEqual(next_page['items'][0]['message']['payload']['parameters'], [{'u64': 2**64-1}, {'i64': -2**63}, {'bool': False}])
        self.assertNotIn('deliveries', self.ok(f, 'mynofy_get_message "$1"', first['message_id']))
        self.assertNotIn('deliveries', self.ok(f, 'mynofy_get_message "$1"', second['message_id']))
        own = self.ok(f, 'mynofy_list_messages local/events "" 1')
        self.assertEqual(len(self.ok(f, 'mynofy_list_messages local/events "$1" 1', own['next_cursor'])['items']), 1)
        self.assertEqual(self.ok(f, 'mynofy_poll local/events "$1"', next_page['next_cursor'])['items'], [])
        denied = dict(f.env, MYNOFY_KEY=str(f.dir / 'other.pk8'))
        p = call('mynofy_publish_text local/events forbidden', env=denied)
        self.assertEqual(p.returncode, 10, p.stderr);self.assertEqual(json.loads(p.stdout)['code'], 'forbidden')

    def test_rotation_and_keygen(self):
        f = self.fixture('unix')
        extra = f.dir / 'extra.pk8'
        created = self.ok(f, 'mynofy_keygen "$1"', str(extra))
        self.assertEqual(created['algorithm'], 'ed25519');self.assertEqual(extra.stat().st_mode & 0o777, 0o600)
        sent = self.ok(f, 'mynofy_publish_text local/events before')
        fresh = f.dir / 'fresh.pk8'
        rotated = self.ok(f, 'mynofy_rotate "$1"', str(fresh))
        self.assertEqual(rotated['client_id'], f.key['client_id'])
        self.assertNotEqual(call('mynofy_get_catalog', env=f.env).returncode, 0)
        env = dict(f.env, MYNOFY_KEY=str(fresh), MYNOFY_CLIENT_ID=f.key['client_id'])
        self.assertEqual(self.ok(f, 'mynofy_get_message "$1"', sent['message_id'], env=env)['message']['payload']['data'], base64.b64encode(b'before').decode())
        self.assertEqual(call('mynofy_rotate "$1"', str(fresh), env=env).returncode, 20)
        self.assertEqual(self.ok(f, 'mynofy_get_catalog', env=env)['catalog']['name'], 'mynofy')

    def test_batch_session_and_tls_failures(self):
        f = self.fixture('wss')
        requests = '\n'.join(json.dumps(dict(version=1, type='get_catalog', id=str(i), name='mynofy', catalog_version=1)) for i in range(3))
        p = call('printf "%s\\n" "$1" | mynofy_session', requests, env=f.env)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual([json.loads(line)['request_id'] for line in p.stdout.splitlines()], ['0', '1', '2'])
        env = dict(f.env, MYNOFY_URL=f.env['MYNOFY_URL'].replace('localhost', '127.0.0.1'))
        self.assertEqual(call('mynofy_get_catalog', env=env).returncode, 20)
        env = dict(f.env, MYNOFY_URL=f.env['MYNOFY_URL'].replace('wss:', 'ws:'))
        self.assertEqual(call('mynofy_get_catalog', env=env).returncode, 64)


if __name__ == '__main__':
    unittest.main()
