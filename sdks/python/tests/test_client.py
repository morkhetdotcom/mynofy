import asyncio
import base64
from contextlib import asynccontextmanager
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest

from google.protobuf import json_format
from websockets.asyncio.server import serve
from websockets.exceptions import ConnectionClosed
from mynofy import BrokerError, BusyError, Client, Event, Key, Parameter, ProtocolError, TransportError, pb
from mynofy.keys import auth_bytes, b64, decode

ROOT = Path(__file__).resolve().parents[3]
BIN = Path(os.environ.get('MYNOFY_BIN', ROOT / 'target/debug/mynofy'))
TOPIC = 'local/events'


class Vectors(unittest.TestCase):
    def test_boot_identity_shared_vectors(self):
        from mynofy.client import _validate_boot_id
        vectors = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())
        for row in vectors['boot_identity']:
            with self.subTest(row=row['name']):
                if row['valid']:
                    self.assertEqual(_validate_boot_id(row['value']), row['value'])
                else:
                    with self.assertRaises(ProtocolError):
                        _validate_boot_id(row['value'])

    def test_wire_and_auth_vectors_are_shared_with_rust(self):
        vectors = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())
        def convert(value):
            if isinstance(value, list):
                return [convert(v) for v in value]
            if isinstance(value, dict):
                if value.get('kind') == 'event':
                    return {'event': {k: convert(v) for k, v in value.items() if k != 'kind'}}
                if value.get('kind') == 'bytes':
                    return {k: v for k, v in value.items() if k != 'kind'}
                return {k: convert(v) for k, v in value.items()}
            return value
        for item in vectors['frames']:
            shape = dict(item['json'])
            kind = shape.pop('type'); version = shape.pop('version')
            frame = json_format.ParseDict({'version': version, kind: convert(shape)}, pb.Frame())
            self.assertEqual(b64(frame.SerializeToString()), item['protobuf_base64'])
            self.assertEqual(pb.Frame.FromString(decode(item['protobuf_base64'])), frame)
        for item in vectors['auth']:
            self.assertEqual(b64(auth_bytes(decode(item['nonce']), decode(item['public_key']), item['owner'])), item['transcript_base64'])
        item = vectors['rotation']
        aad = json.dumps(['mynofy-rotation-v1', item['owner'], item['client_id'],
                          item['rotation_id'], item['old_token'], item['until_ms']],
                         separators=(',', ':'), ensure_ascii=False).encode()
        self.assertEqual(b64(aad), item['aad_base64'])
        self.assertEqual(b64(b'mynofy-rotation-proof-v1\0' + aad + decode(item['public_key']) + decode(item['challenge'])), item['proof_base64'])

    def test_shared_payload_validation(self):
        from mynofy.client import _payload
        vectors = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())
        for item in vectors['payload_validation']:
            with self.subTest(item=item['name']):
                payload = json_format.ParseDict(item['payload'], pb.Payload())
                value = payload.event if payload.WhichOneof('body') == 'event' else payload.data
                content_type = payload.content_type if payload.HasField('content_type') else None
                if item['valid']:
                    self.assertEqual(_payload(value, content_type), payload)
                else:
                    with self.assertRaises(ValueError):
                        _payload(value, content_type)
                    client = Client(None, Key.generate(), 'local', None, 1)
                    with self.assertRaises(ValueError):
                        asyncio.run(client.publish(TOPIC, value, content_type=content_type))

    def test_key_formats_and_invalid_embedded_public_key(self):
        key = Key.generate()
        self.assertEqual(Key(key.private_bytes()).public_key, key.public_key)
        # Same exact RFC 5958 structure emitted by the Rust CLI.
        from cryptography.hazmat.primitives import serialization
        seed = key._key.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
        v2 = bytes.fromhex('3051020101300506032b657004220420') + seed + b'\x81\x21\0' + key.public_key
        self.assertEqual(Key(v2).public_key, key.public_key)
        with self.assertRaises(ValueError):
            Key(v2[:-1] + bytes([v2[-1] ^ 1]))
        with self.assertRaises(ValueError):
            Key(v2 + b'\0')


class Fixture:
    def __init__(self, mode):
        self.temp = tempfile.TemporaryDirectory(prefix='mn-py-')
        self.dir = Path(self.temp.name)
        self.mode = mode
        self.proc = None
        # Read a real Rust-created key, not only Python-created key formats.
        result = json.loads(subprocess.check_output([str(BIN), 'keygen', '--out', str(self.dir / 'rust.pk8')]))
        self.key = Key((self.dir / 'rust.pk8').read_bytes())
        assert self.key.client_id == result['client_id']
        self.other = Key.generate()
        cfg = json.loads((ROOT / 'config/scenarios/unix.json').read_text())
        self.socket = self.dir / 'm.sock'
        cfg['service'] = {'id': 'local', 'unix_socket': str(self.socket)}
        if mode == 'wss':
            subprocess.run(['openssl', 'req', '-x509', '-newkey', 'ec', '-pkeyopt',
                            'ec_paramgen_curve:P-256', '-nodes', '-days', '1',
                            '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost',
                            '-keyout', str(self.dir / 's.key'), '-out', str(self.dir / 'ca.pem')],
                           check=True, capture_output=True)
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0)); port = probe.getsockname()[1]
            self.port = port
            self.url = f'wss://localhost:{port}'
            cfg['service'] = {'id': 'local', 'listen': f'127.0.0.1:{port}',
                              'tls': {'certificate_file': str(self.dir / 'ca.pem'),
                                      'private_key_file': str(self.dir / 's.key')}}
        cfg['clients'] = [dict(client_id=key.client_id,
                               keys=[dict(algorithm='ed25519', public_key=b64(key.public_key),
                                          valid_until='2035-01-01T00:00:00Z')])
                          for key in [self.key, self.other]]
        cfg['topics'][0]['acl'] = [dict(client_id=self.key.client_id,
                                       allow=['publish', 'subscribe', 'inspect_own'])]
        cfg['limits']['client_burst'] = 1000
        cfg['limits']['client_requests_per_second'] = 1000
        cfg['limits']['auth_timeout_seconds'] = 1
        self.config = self.dir / 'config.json'
        cfg["security_storage"] = {"directory": "security"}
        self.config.write_text(json.dumps(cfg))
        subprocess.run([str(BIN), "security-init", "--config", str(self.config)], check=True)

    async def start(self):
        self.proc = subprocess.Popen([str(BIN), 'serve', '--config', str(self.config)],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        for _ in range(200):
            if self.proc.poll() is not None:
                raise RuntimeError(self.proc.stderr.read().decode())
            if self.mode == 'unix' and self.socket.exists():
                return
            if self.mode == 'wss':
                try:
                    reader, writer = await asyncio.open_connection('127.0.0.1', self.port)
                    writer.close(); await writer.wait_closed(); return
                except OSError:
                    pass
            await asyncio.sleep(.01)
        raise RuntimeError('Service startup timeout')

    async def connect(self, key=None, **kwargs):
        if self.mode == 'unix':
            return await Client.connect_unix(self.socket, owner='local', key=key or self.key, **kwargs)
        return await Client.connect_wss(self.url, owner='local', key=key or self.key,
                                        ca_file=self.dir / 'ca.pem', **kwargs)

    def stop(self):
        if self.proc is not None:
            if self.proc.poll() is None:
                self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill(); self.proc.wait(timeout=5)
            self.proc.stderr.close()
        self.temp.cleanup()


@asynccontextmanager
async def service(mode='unix'):
    fixture = Fixture(mode)
    try:
        await fixture.start()
        yield fixture
    finally:
        fixture.stop()


class Integration(unittest.IsolatedAsyncioTestCase):
    async def test_real_parallel_capacity_rejection_is_correlated_and_reusable(self):
        for mode in ['unix', 'wss']:
            with self.subTest(mode=mode):
                f = Fixture(mode); clients = []; slow = None
                try:
                    cfg = json.loads(f.config.read_text())
                    cfg['limits'].update(max_parallel_requests_per_client=1,
                        max_response_bytes=4*1024*1024, client_burst=1000000,
                        client_requests_per_second=1000000)
                    f.config.write_text(json.dumps(cfg))
                    await f.start()
                    producer = await f.connect(); clients.append(producer)
                    for _ in range(100):
                        await producer.publish(TOPIC, b'x'*4096)
                    slow = await f.connect(); clients.append(slow)
                    slow._ws.transport.pause_reading()
                    pages = 64  # Exceed TCP send buffering as well as Unix buffering.
                    for i in range(pages):
                        await slow._ws.send(pb.Frame(version=1, poll=pb.Poll(
                            id=f'slow-{i}', topic=TOPIC, limit=100)).SerializeToString())
                    probe = await f.connect(); clients.append(probe)
                    original_socket = probe._ws
                    async with asyncio.timeout(5):
                        while True:
                            try:
                                await probe.get_permissions(TOPIC)
                            except BrokerError as error:
                                self.assertEqual(error.code, 'resource_exhausted')
                                break
                            await asyncio.sleep(.01)
                    with self.assertRaises(BrokerError) as rejected:
                        await probe.publish(TOPIC, b'must not be published')
                    self.assertEqual(rejected.exception.code, 'resource_exhausted')
                    self.assertIs(probe._ws, original_socket)
                    slow._ws.transport.resume_reading()
                    async with asyncio.timeout(10):
                        for i in range(pages):
                            frame = pb.Frame.FromString(await slow._ws.recv())
                            self.assertEqual(frame.messages.request_id, f'slow-{i}')
                            self.assertEqual(len(frame.messages.items), 100)
                    async with asyncio.timeout(5):
                        while True:
                            try:
                                existing = await probe.list_messages(topic=TOPIC, limit=100)
                                break
                            except BrokerError as error:
                                self.assertEqual(error.code, 'resource_exhausted')
                                await asyncio.sleep(.01)
                    self.assertEqual(len(existing.items), 100)
                    self.assertFalse(existing.HasField('next_cursor'))
                    await probe.publish(TOPIC, b'explicit publish after capacity release')
                    self.assertIs(probe._ws, original_socket)
                finally:
                    if slow is not None and slow._ws is not None:
                        slow._ws.transport.resume_reading()
                    for client in clients:
                        await client.close()
                    f.stop()

    async def test_native_unix_and_wss_roundtrip_with_all_basic_operations(self):
        for mode in ['unix', 'wss']:
            async with service(mode) as f:
                async with await f.connect() as client:
                    self.assertEqual(client.info.client_id, f.key.client_id)
                    self.assertIsInstance(client.info.restart_counter, int)
                    self.assertEqual(len(decode(client.info.boot_id, 16)), 16)
                    catalog = await client.get_catalog()
                    self.assertEqual(catalog['catalog_id'], 1)
                    rights = await client.get_permissions(TOPIC)
                    self.assertIn('publish', rights.actions)
                    binary = b'\0\xffhello'
                    first = await client.publish(TOPIC, binary, content_type='application/octet-stream')
                    event = Event(catalog=1, catalog_version=1, category=12, action=42, status=2,
                                  parameters=[Parameter(u64=2**64-1), Parameter(i64=-(2**63)), Parameter(bool=False)])
                    second = await client.publish(TOPIC, event)
                    page = await client.poll(TOPIC, limit=1)
                    self.assertTrue(page.has_more)
                    self.assertEqual(page.items[0].message.payload.data, binary)
                    self.assertEqual(page.items[0].message.producer_id, client.info.client_id)
                    page2 = await client.poll(TOPIC, after=page.next_cursor)
                    self.assertEqual(page2.items[0].message.payload.event, event)
                    own = await client.list_messages(topic=TOPIC, limit=1)
                    self.assertEqual(len(own.items), 1)
                    self.assertTrue(own.HasField('next_cursor'))
                    own2 = await client.list_messages(topic=TOPIC, cursor=own.next_cursor, limit=1)
                    self.assertEqual(len(own2.items), 1)
                    self.assertEqual((await client.get_message(first.message_id)).message.id, first.message_id)
                    self.assertEqual((await client.get_message(second.message_id)).message.id, second.message_id)
                    self.assertFalse((await client.poll(TOPIC, after=page2.next_cursor)).items)
                    self.assertEqual(len((await client.poll(TOPIC)).items), 2)
                async with await f.connect(f.other) as denied:
                    with self.assertRaises(BrokerError) as failure:
                        await denied.publish(TOPIC, b'forbidden')
                    self.assertEqual(failure.exception.code, 'forbidden')
                    self.assertEqual((await denied.get_catalog())['name'], 'mynofy')

    async def test_payload_validation_and_expiry_are_not_silent_coercions(self):
        async with service() as f:
            async with await f.connect() as client:
                for value in [b'x' * 4097, Event(catalog=1, catalog_version=1, category=999, action=42, status=2)]:
                    with self.assertRaises(ValueError):
                        await client.publish(TOPIC, value)
                with self.assertRaises(TypeError):
                    await client.publish(TOPIC, 'implicit text')
                with self.assertRaises(ValueError):
                    await client.publish(TOPIC, b'test', expires=datetime.now())
                self.assertFalse((await client.poll(TOPIC)).items)
                await client.publish(TOPIC, b'x' * 4096, expires=datetime.now(timezone.utc) + timedelta(seconds=30))
                self.assertEqual(len((await client.poll(TOPIC)).items), 1)

    async def test_python_generated_key_is_accepted_by_rust_cli_and_rotation_preserves_identity(self):
        async with service() as f:
            async with await f.connect() as client:
                published = await client.publish(TOPIC, b'before rotation')
                fresh = Key.generate()
                # Caller owns key persistence before rotation; SDK never rewrites key files.
                path = f.dir / 'new.pk8'; path.write_bytes(fresh.private_bytes()); path.chmod(0o600)
                result = await client.rotate(fresh)
                self.assertEqual(result.client_id, f.key.client_id)
            with self.assertRaises(BrokerError):
                await f.connect()  # Old key revoked.
            async with await f.connect(fresh, client_id=f.key.client_id) as rotated:
                restored = (await rotated.get_message(published.message_id)).message
                self.assertEqual(restored.payload.data, b'before rotation')
                self.assertEqual(restored.producer_id, f.key.client_id)
            command = [str(BIN), 'client', '--socket', str(f.socket), '--owner', 'local',
                       '--key', str(path), '--client-id', f.key.client_id]
            proc = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                                                        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            stdout, stderr = await proc.communicate(b'{"version":1,"type":"get_catalog","id":"rust","name":"mynofy","catalog_version":1}\n')
            self.assertEqual(proc.returncode, 0, stderr.decode())
            self.assertEqual(json.loads(stdout)['type'], 'catalog')

    async def test_wrong_owner_unknown_key_and_wrong_expected_identity_are_rejected(self):
        async with service() as f:
            with self.assertRaises(ProtocolError):
                await Client.connect_unix(f.socket, owner='wrong', key=f.key)
            with self.assertRaises(BrokerError):
                await f.connect(Key.generate())
            with self.assertRaises(ProtocolError):
                await f.connect(client_id='client:wrong')
            async with await f.connect() as client:
                self.assertEqual(len((await client.poll(TOPIC)).items), 0)

    async def test_wss_rejects_redirect_before_contacting_target(self):
        import ssl
        from http import HTTPStatus
        from websockets.exceptions import InvalidStatus

        fixture = Fixture('wss')
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(fixture.dir/'ca.pem', fixture.dir/'s.key')
        target_contacted = False

        def target_request(connection, request):
            nonlocal target_contacted
            target_contacted = True
            return connection.respond(HTTPStatus.FORBIDDEN, 'unexpected request')

        async def unused_handler(_):
            self.fail('unexpected authenticated upgrade')

        try:
            async with serve(unused_handler, '127.0.0.1', 0, ssl=context,
                             process_request=target_request) as target:
                port = target.sockets[0].getsockname()[1]

                def redirect(connection, request):
                    response = connection.respond(HTTPStatus.TEMPORARY_REDIRECT, 'redirect')
                    response.headers['Location'] = f'wss://localhost:{port}/'
                    return response

                async with serve(unused_handler, '127.0.0.1', 0, ssl=context,
                                 process_request=redirect) as origin:
                    port = origin.sockets[0].getsockname()[1]
                    with self.assertRaises(InvalidStatus) as raised:
                        await Client.connect_wss(f'wss://localhost:{port}/', owner='local',
                                                 key=fixture.key, ca_file=fixture.dir/'ca.pem')
                    self.assertEqual(raised.exception.response.status_code, HTTPStatus.TEMPORARY_REDIRECT)
        finally:
            fixture.stop()
        self.assertFalse(target_contacted)

    async def test_wss_rejects_untrusted_ca_and_incorrect_hostname(self):
        async with service('wss') as f:
            # A certificate for localhost and 127.0.0.1 must not authorize another DNS name.
            import ssl
            with self.assertRaises(ssl.SSLCertVerificationError):
                await Client.connect_wss(f.url, owner='local', key=f.key, ca_file=None)
            with self.assertRaises(ssl.SSLCertVerificationError):
                await Client.connect_wss(f.url.replace('localhost', '127.0.0.1'), owner='local', key=f.key, ca_file=f.dir/'ca.pem')
            with self.assertRaises(ValueError):
                await Client.connect_wss(f.url.replace('wss:', 'ws:'), owner='local', key=f.key, ca_file=f.dir/'ca.pem')


class TransportFailures(unittest.IsolatedAsyncioTestCase):
    async def test_cli_bash_and_mcp_share_reply_semantics(self):
        from websockets.asyncio.server import unix_serve
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
        vectors = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())
        key = Key.generate()
        with tempfile.TemporaryDirectory(prefix='mn-replies-') as raw:
            folder = Path(raw); path = folder / 'peer.sock'; keyfile = folder / 'key.pk8'
            keyfile.write_bytes(key.private_bytes()); keyfile.chmod(0o600)
            for case in vectors['reply_validation']:
                for surface in ['cli', 'bash', 'mcp']:
                    with self.subTest(case=case['name'], surface=surface):
                        received = []
                        async def peer(ws):
                            nonce = os.urandom(32)
                            hello = pb.Frame.FromString(await ws.recv()).auth_hello
                            self.assertEqual(hello.token_id, key.token_id)
                            await ws.send(pb.Frame(version=1, auth_challenge=pb.AuthChallenge(
                                request_id=hello.id, service_id='owner', nonce=b64(nonce))).SerializeToString())
                            proof = pb.Frame.FromString(await ws.recv()).auth_proof
                            self.assertEqual(proof.auth_id, hello.id)
                            Ed25519PublicKey.from_public_bytes(key.public_key).verify(
                                decode(proof.signature, 64), auth_bytes(nonce, key.public_key, 'owner'))
                            await ws.send(pb.Frame(version=1, auth_welcome=pb.AuthWelcome(
                                request_id=proof.id, service_id='owner', client_id=key.client_id,
                                restart_counter=1)).SerializeToString())
                            try:
                                async for data in ws:
                                    request = pb.Frame.FromString(data); received.append(request)
                                    response = pb.Frame.FromString(base64.b64decode(case['response_base64']))
                                    getattr(response, response.WhichOneof('body')).request_id = getattr(
                                        request, request.WhichOneof('body')).id
                                    await ws.send(response.SerializeToString())
                            except ConnectionClosed:
                                pass
                        async with unix_serve(peer, path=str(path), ping_interval=None):
                            path.chmod(0o600)
                            common = ['--socket', str(path), '--key', str(keyfile), '--owner', 'owner']
                            if surface == 'bash':
                                command = 'mynofy_publish_text local/events once' if case['mutation'] else 'mynofy_get_message 00000001'
                                args = ['bash', '-c', 'source "$1"\n' + command, 'test', str(ROOT / 'sdks/bash/mynofy.sh')]
                            else:
                                args = [str(BIN), 'mcp' if surface == 'mcp' else 'client', *common]
                            process = await asyncio.create_subprocess_exec(*args, stdin=asyncio.subprocess.PIPE,
                                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                                env=dict(os.environ, MYNOFY_BIN=str(BIN), MYNOFY_SOCKET=str(path),
                                         MYNOFY_KEY=str(keyfile), MYNOFY_OWNER='owner'))
                            try:
                                if surface == 'mcp':
                                    args = {k: v for k, v in case['request'].items() if k not in ('version', 'type', 'id')}
                                    request = dict(jsonrpc='2.0', id=1, method='tools/call', params=dict(
                                        name='mynofy_publish' if case['mutation'] else 'mynofy_get_message', arguments=args,
                                        _meta={'io.modelcontextprotocol/protocolVersion': '2026-07-28',
                                               'io.modelcontextprotocol/clientCapabilities': {}}))
                                    process.stdin.write((json.dumps(request)+'\n').encode()); await process.stdin.drain()
                                    line = await asyncio.wait_for(process.stdout.readline(), 5)
                                    result = json.loads(line)['result']
                                    self.assertEqual(result['isError'], not case['valid'])
                                    if not case['valid']:
                                        self.assertEqual(result['structuredContent']['code'], 'transport_error')
                                        self.assertEqual(result['structuredContent']['outcome_unknown'], case['mutation'])
                                    process.stdin.close()
                                    await asyncio.wait_for(process.wait(), 5)
                                else:
                                    data = (json.dumps(case['request'])+'\n').encode() if surface == 'cli' else b''
                                    output, errors = await asyncio.wait_for(process.communicate(data), 10)
                                    self.assertEqual(process.returncode == 0, case['valid'], errors.decode())
                                    if case['valid']:
                                        result = json.loads(output)
                                        self.assertEqual(result['type'], case['response']['type'])
                                    else:
                                        self.assertEqual(output, b'')
                                self.assertEqual(len(received), 1, 'unexpected retry')
                            finally:
                                if process.returncode is None:
                                    process.kill(); await process.wait()

    async def test_shared_reply_semantics(self):
        from websockets.asyncio.client import connect
        vectors = json.loads((ROOT / 'protocol/conformance/sdk-v1.json').read_text())
        for case in vectors['reply_validation']:
            with self.subTest(case=case['name']):
                received = []
                async def owner(ws):
                    try:
                        async for raw in ws:
                            request = pb.Frame.FromString(raw)
                            received.append(request)
                            response = pb.Frame.FromString(base64.b64decode(case['response_base64']))
                            reply = getattr(response, response.WhichOneof('body'))
                            reply.request_id = getattr(request, request.WhichOneof('body')).id
                            await ws.send(response.SerializeToString())
                    except ConnectionClosed:
                        pass
                async with serve(owner, '127.0.0.1', 0) as server:
                    ws = await connect(f'ws://127.0.0.1:{server.sockets[0].getsockname()[1]}')
                    client = Client(ws, Key.generate(), 'owner', None, 1)
                    async def operation():
                        if case['mutation']:
                            return await client.publish(TOPIC, b'once')
                        return await client.get_message('00000001')
                    if case['valid']:
                        await operation(); await operation()
                        self.assertEqual(len(received), 2)
                    else:
                        with self.assertRaises(TransportError) as error:
                            await operation()
                        self.assertEqual(error.exception.outcome_unknown, case['mutation'])
                        self.assertIsNone(client._ws)
                        with self.assertRaises(TransportError):
                            await operation()
                        self.assertEqual(len(received), 1)
                    await client.close()

    async def test_uncorrelated_rate_limit_preserves_connection_and_backoff(self):
        async def owner(ws):
            for _ in range(2):
                await ws.recv()
                await ws.send(pb.Frame(version=1, error=pb.Error(
                    code='rate_limited', detail='wait', retry_after_ms=123)).SerializeToString())
        async with serve(owner, '127.0.0.1', 0) as server:
            from websockets.asyncio.client import connect
            ws = await connect(f'ws://127.0.0.1:{server.sockets[0].getsockname()[1]}')
            client = Client(ws, Key.generate(), 'owner', None, 1)
            for _ in range(2):
                with self.assertRaises(BrokerError) as failure:
                    await client.publish(TOPIC, b'rejected')
                self.assertEqual(failure.exception.retry_after_ms, 123)
                self.assertIs(client._ws, ws)
            await client.close()

    async def test_lost_publish_response_closes_socket_and_never_retries(self):
        received = []
        async def owner(ws):
            received.append(pb.Frame.FromString(await ws.recv()))
            await ws.close()
        async with serve(owner, '127.0.0.1', 0) as server:
            from websockets.asyncio.client import connect
            port = server.sockets[0].getsockname()[1]
            ws = await connect(f'ws://127.0.0.1:{port}')
            client = Client(ws, Key.generate(), 'owner', None, 1)
            with self.assertRaises(TransportError) as error:
                await client.publish(TOPIC, b'accepted but reply lost')
            self.assertTrue(error.exception.outcome_unknown)
            with self.assertRaises(TransportError):
                await client.publish(TOPIC, b'not retried')
            self.assertEqual(len(received), 1)

    async def test_cancellation_discards_stream_and_busy_call_is_not_queued(self):
        started = asyncio.Event(); closed = asyncio.Event()
        async def owner(ws):
            await ws.recv(); started.set()
            await ws.wait_closed(); closed.set()
        async with serve(owner, '127.0.0.1', 0) as server:
            from websockets.asyncio.client import connect
            ws = await connect(f'ws://127.0.0.1:{server.sockets[0].getsockname()[1]}')
            client = Client(ws, Key.generate(), 'owner', None, 1)
            pending = asyncio.create_task(client.poll(TOPIC))
            await asyncio.wait_for(started.wait(), 2)
            with self.assertRaises(BusyError):
                await client.publish(TOPIC, b'not sent')
            pending.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await pending
            await asyncio.wait_for(closed.wait(), 2)
            self.assertIsNone(client._ws)

    async def test_close_during_inflight_request_discards_stream_cleanly(self):
        started = asyncio.Event()
        async def owner(ws):
            await ws.recv(); started.set(); await ws.wait_closed()
        async with serve(owner, '127.0.0.1', 0) as server:
            from websockets.asyncio.client import connect
            ws = await connect(f'ws://127.0.0.1:{server.sockets[0].getsockname()[1]}')
            client = Client(ws, Key.generate(), 'owner', None, 1)
            pending = asyncio.create_task(client.publish(TOPIC, b'pending'))
            await asyncio.wait_for(started.wait(), 2)
            await client.close()
            with self.assertRaises(TransportError) as error:
                await pending
            self.assertTrue(error.exception.outcome_unknown)

    async def test_timeout_and_mismatched_reply_discard_connection(self):
        for mode in ['timeout', 'wrong-id', 'invalid-version']:
            async def owner(ws):
                request = pb.Frame.FromString(await ws.recv())
                if mode == 'timeout':
                    await ws.wait_closed()
                else:
                    response = pb.Frame(version=2 if mode == 'invalid-version' else 1,
                                        result=pb.Result(request_id='wrong-id'))
                    await ws.send(response.SerializeToString())
            async with serve(owner, '127.0.0.1', 0) as server:
                from websockets.asyncio.client import connect
                ws = await connect(f'ws://127.0.0.1:{server.sockets[0].getsockname()[1]}')
                client = Client(ws, Key.generate(), 'owner', None, .1)
                with self.assertRaises(TransportError) as error:
                    await client.publish(TOPIC, b'test')
                self.assertTrue(error.exception.outcome_unknown)
                self.assertIsNone(client._ws)


if __name__ == '__main__':
    unittest.main()
