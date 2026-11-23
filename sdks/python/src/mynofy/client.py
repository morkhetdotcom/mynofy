"""Native asyncio SDK. No subprocess, workflow engine or automatic mutation retry."""
import asyncio
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
import json
import math
import os
from pathlib import Path
import socket
import ssl
import stat
import sys
from urllib.parse import urlsplit

from google.protobuf.message import DecodeError
from websockets.asyncio.client import connect, unix_connect
from websockets.exceptions import ConnectionClosed

from . import mynofy_pb2 as pb
from ._catalog import CATEGORIES, ACTIONS, STATUSES
from .keys import Key, auth_bytes, b64, decode


class ProtocolError(Exception):
    pass


class BusyError(Exception):
    pass


class TransportError(Exception):
    def __init__(self, message: str, *, outcome_unknown: bool = False):
        super().__init__(message)
        self.outcome_unknown = outcome_unknown


class BrokerError(Exception):
    def __init__(self, error: pb.Error):
        super().__init__(f'{error.code}: {error.detail}')
        self.code, self.detail = error.code, error.detail
        self.retry_after_ms = error.retry_after_ms if error.HasField('retry_after_ms') else None


@dataclass(frozen=True)
class ConnectionInfo:
    owner: str
    client_id: str
    restart_counter: int


def _strict_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ProtocolError('Duplicate JSON key')
        value[key] = item
    return value


def _json(data):
    def invalid(value):
        raise ProtocolError('Invalid JSON constant: ' + value)
    return json.loads(data, object_pairs_hook=_strict_object, parse_constant=invalid)


def _topic(topic):
    if not isinstance(topic, str) or not 1 <= len(topic.encode()) <= 256:
        raise ValueError('Topic must contain 1..256 UTF-8 bytes')
    return topic


def _time(value):
    if not isinstance(value, datetime) or value.utcoffset() is None:
        raise ValueError('Expected timezone-aware datetime')
    return value.astimezone(timezone.utc).isoformat(timespec='milliseconds').replace('+00:00', 'Z')


def _payload(value, content_type):
    if isinstance(value, bytes):
        if len(value) > 4096:
            raise ValueError('Payload exceeds 4096 bytes')
        result = pb.Payload(data=value)
    elif isinstance(value, pb.Event):
        if content_type is not None:
            raise ValueError('Event payloads must not have a content type')
        if (not value.IsInitialized() or value.catalog != 1 or value.catalog_version != 1
                or value.category not in CATEGORIES or value.action not in ACTIONS
                or value.status not in STATUSES or len(value.parameters) > 3):
            raise ValueError('Invalid catalog event')
        for parameter in value.parameters:
            name = parameter.WhichOneof('value')
            if name is None or name in ('f32', 'f64') and not math.isfinite(getattr(parameter, name)):
                raise ValueError('Invalid event parameter')
        result = pb.Payload(event=value)
    else:
        raise TypeError('Payload must be bytes or Event; encode text explicitly as UTF-8')
    if content_type is not None:
        if not isinstance(content_type, str) or not 1 <= len(content_type.encode()) <= 128:
            raise ValueError('Content type must contain 1..128 UTF-8 bytes')
        result.content_type = content_type
    return result


class Client:
    def __init__(self, ws, key, owner, expected_client, timeout):
        self._ws, self._key, self._owner = ws, key, owner
        self._expected_client = expected_client or key.client_id
        self._timeout, self._busy, self._sequence = timeout, False, 0
        self.info = None

    @staticmethod
    def _options(owner, key, timeout):
        _topic(owner)
        if not isinstance(key, Key):
            raise TypeError('key must be Key')
        if not math.isfinite(timeout) or timeout <= 0:
            raise ValueError('timeout must be finite and positive')
        return dict(open_timeout=timeout, close_timeout=min(timeout, 2), ping_interval=None,
                    compression=None, max_size=4 * 1024 * 1024, max_queue=1)

    @classmethod
    async def connect_wss(cls, url, *, owner, key, ca_file, client_id=None, timeout=10):
        options = cls._options(owner, key, timeout)
        uri = urlsplit(url)
        if uri.scheme != 'wss' or not uri.hostname or uri.username or uri.password or uri.fragment:
            raise ValueError('Expected wss:// URL without credentials or fragment')
        context = ssl.create_default_context(cafile=str(ca_file) if ca_file is not None else None)
        context.minimum_version = ssl.TLSVersion.TLSv1_3
        # Do not inherit ambient HTTP/SOCKS proxies or weaken CA/hostname checks.
        ws = await connect(url, ssl=context, proxy=None, **options)
        return await cls._authenticated(ws, key, owner, client_id, timeout)

    @classmethod
    async def connect_unix(cls, path, *, owner, key, client_id=None, timeout=10):
        options = cls._options(owner, key, timeout)
        if os.name != 'posix':
            raise ValueError('Unix sockets require a Unix platform')
        path = Path(path)
        parent = path.parent.resolve(strict=True)
        if parent.stat().st_mode & 0o022:
            raise ValueError('Socket parent must not be writable by group/others')
        path = parent / path.name
        metadata = path.lstat()
        if not stat.S_ISSOCK(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o600 or metadata.st_uid != os.getuid():
            raise ValueError('Expected a same-UID Unix socket with mode 0600')
        # Check the connected peer, too, to detect endpoint replacement races.
        stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stream.setblocking(False)
        try:
            async with asyncio.timeout(timeout):
                await asyncio.get_running_loop().sock_connect(stream, str(path))
            uid = _peer_uid(stream)
            if uid != metadata.st_uid:
                raise ValueError('Unix peer UID mismatch')
            ws = await unix_connect(sock=stream, **options)
        except BaseException:
            stream.close()
            raise
        return await cls._authenticated(ws, key, owner, client_id, timeout)

    @classmethod
    async def _authenticated(cls, ws, key, owner, client_id, timeout):
        self = cls(ws, key, owner, client_id, timeout)
        try:
            hello = await self._request('auth_hello', pb.AuthHello(token_id=key.token_id), 'auth_challenge')
            if hello.service_id != owner:
                raise ProtocolError('Owner challenge mismatch')
            signature = b64(key.sign(auth_bytes(decode(hello.nonce, 32), key.public_key, owner)))
            # Authentication changes session state: a failed exchange discards the socket.
            proof = await self._request('auth_proof', pb.AuthProof(auth_id=hello.request_id, signature=signature), 'auth_welcome')
            if proof.service_id != owner or proof.client_id != self._expected_client or not proof.HasField('restart_counter'):
                raise ProtocolError('Owner/client welcome mismatch')
            self.info = ConnectionInfo(owner, proof.client_id, proof.restart_counter)
            return self
        except BaseException:
            await self.close()
            raise

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self.close()

    async def close(self):
        ws, self._ws = self._ws, None
        if ws is not None:
            # Close is bounded by the configured WebSocket close_timeout.
            await ws.close()

    async def _request(self, kind, body, expected, *, mutation=False):
        if self._busy:
            raise BusyError('One operation per client; use another connection for concurrent work')
        self._busy = True
        try:
            return await self._roundtrip(kind, body, expected, mutation=mutation)
        finally:
            self._busy = False

    async def _roundtrip(self, kind, body, expected, *, mutation=False):
        if self._ws is None:
            raise TransportError('Client is closed')
        ws = self._ws
        self._sequence += 1
        body.id = 'py-' + str(self._sequence)
        frame = pb.Frame(version=1)
        getattr(frame, kind).CopyFrom(body)
        encoded = frame.SerializeToString()  # Validate before any network side effect.
        try:
            async with asyncio.timeout(self._timeout):
                await ws.send(encoded)
                raw = await ws.recv()
                if not isinstance(raw, bytes):
                    raise ProtocolError('Expected Protobuf binary response')
                response = pb.Frame.FromString(raw)
                name = response.WhichOneof('body')
                if not response.IsInitialized() or response.version != 1 or name not in (expected, 'error'):
                    raise ProtocolError('Invalid response envelope')
                result = getattr(response, name)
                early_rate_limit = (name == 'error' and result.code == 'rate_limited'
                                    and not result.HasField('request_id')
                                    and result.HasField('retry_after_ms'))
                if not early_rate_limit and (not result.HasField('request_id') or result.request_id != body.id):
                    raise ProtocolError('Response request ID mismatch')
                messages = ([item.message for item in result.items] if name == 'messages'
                            else [result.message] if name == 'message_status' else [])
                if any(not message.producer_id for message in messages):
                    raise ProtocolError('Missing owner-authenticated producer identity')
                if name == 'error':
                    raise BrokerError(result)
                if kind == 'publish' and (not result.HasField('message_id') or not result.HasField('expires')):
                    raise ProtocolError('Incomplete publish result')
                if kind == 'get_message' and result.message.id != body.message_id:
                    raise ProtocolError('Response message ID mismatch')
                return result
        except BrokerError:
            raise
        except asyncio.CancelledError:
            # A cancelled in-flight request must never consume a later response.
            self._ws = None
            ws.transport.abort()
            raise
        except (OSError, TimeoutError, ConnectionClosed, ProtocolError, DecodeError) as error:
            self._ws = None
            ws.transport.abort()
            raise TransportError('Exchange failed; connection discarded', outcome_unknown=mutation) from error

    async def publish(self, topic, payload, *, content_type=None, expires=None):
        request = pb.Publish(topic=_topic(topic), payload=_payload(payload, content_type))
        if expires is not None:
            request.expires.time = _time(expires)
        return await self._request('publish', request, 'result', mutation=True)

    async def poll(self, topic, *, after=None, limit=20):
        request = pb.Poll(topic=_topic(topic), limit=_limit(limit))
        if after is not None:
            if not isinstance(after, str) or len(after) != 8 or not after.isascii() or not after.isalnum():
                raise ValueError('Cursor must contain eight ASCII alphanumerics')
            request.after = after
        page = await self._request('poll', request, 'messages')
        if len(page.next_cursor) != 8 or not page.next_cursor.isascii() or not page.next_cursor.isalnum() or len(page.items) > limit:
            await self.close()
            raise ProtocolError('Invalid poll page')
        return page

    async def list_messages(self, *, topic=None, cursor=None, limit=20):
        request = pb.ListMessages(limit=_limit(limit))
        if topic is not None:
            request.topic = _topic(topic)
        if cursor is not None:
            if not isinstance(cursor, str) or len(cursor) > 4096:
                raise ValueError('Invalid producer cursor')
            request.cursor = cursor
        return await self._request('list_messages', request, 'messages_page')

    async def get_message(self, message_id):
        if not isinstance(message_id, str) or not message_id or len(message_id.encode()) > 256:
            raise ValueError('Invalid message ID')
        return await self._request('get_message', pb.GetMessage(message_id=message_id), 'message_status')

    async def get_permissions(self, topic):
        return await self._request('get_permissions', pb.GetPermissions(topic=_topic(topic)), 'permissions')

    async def get_catalog(self, *, name='mynofy', version=1):
        result = await self._request('get_catalog', pb.GetCatalog(name=name, catalog_version=version), 'catalog')
        catalog = _json(result.definition)
        if not isinstance(catalog, dict) or catalog.get('name') != name or catalog.get('version') != version:
            raise ProtocolError('Catalog identity mismatch')
        return catalog

    async def rotate(self, new_key: Key):
        if not isinstance(new_key, Key):
            raise TypeError('new_key must be Key; save it before starting rotation')
        if self._busy:
            raise BusyError('Client is busy')
        self._busy = True
        try:
            return await self._rotate(new_key)
        finally:
            self._busy = False

    async def _rotate(self, new_key):
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
        offer = await self._roundtrip('rotation_start', pb.RotationStart(), 'rotation_offer', mutation=True)
        secret = os.urandom(32)
        ready = await self._roundtrip('rotation_confirm', pb.RotationConfirm(rotation_id=offer.rotation_id, shared_secret=b64(secret)), 'rotation_ready', mutation=True)
        if (ready.rotation_id != offer.rotation_id or ready.expires_at != offer.expires_at
                or ready.new_valid_until != offer.new_valid_until):
            await self.close()
            raise ProtocolError('Rotation binding mismatch; retain both keys')
        until = datetime.fromisoformat(ready.new_valid_until.replace('Z', '+00:00'))
        until_ms = (until - datetime(1970, 1, 1, tzinfo=timezone.utc)) // timedelta(milliseconds=1)
        aad = json.dumps(['mynofy-rotation-v1', self._owner, self.info.client_id,
                          offer.rotation_id, self._key.token_id, until_ms],
                         ensure_ascii=False, separators=(',', ':')).encode()
        if decode(ready.aad) != aad:
            await self.close()
            raise ProtocolError('Rotation AAD mismatch; retain both keys')
        encrypted = AESGCM(secret).encrypt(decode(ready.nonce, 12), new_key.public_key, aad)
        proof = b'mynofy-rotation-proof-v1\0' + aad + new_key.public_key + decode(ready.challenge, 32)
        complete = await self._roundtrip('rotation_commit', pb.RotationCommit(rotation_id=offer.rotation_id, encrypted_public_key=b64(encrypted), signature=b64(new_key.sign(proof))), 'rotation_complete', mutation=True)
        await self.close()  # Old session/key is invalidated by the owner.
        if (complete.rotation_id != offer.rotation_id or complete.client_id != self.info.client_id
                or complete.token_id != new_key.token_id or complete.valid_until != ready.new_valid_until):
            raise TransportError('Rotation commit not confirmed; retain both keys', outcome_unknown=True)
        return complete


def _limit(value):
    if type(value) is not int or not 1 <= value <= 100:
        raise ValueError('limit must be an integer in 1..100')
    return value



def _peer_uid(stream):
    if hasattr(socket, 'SO_PEERCRED'):
        import struct
        size = struct.calcsize('3i')
        return struct.unpack('3i', stream.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, size))[1]
    if hasattr(stream, 'getpeereid'):
        return stream.getpeereid()[0]
    if sys.platform == 'darwin':
        # macOS exposes getpeereid in libc, but CPython doesn't expose it on
        # every supported release. uid_t/gid_t are 32-bit unsigned on Darwin.
        import ctypes
        function = ctypes.CDLL(None, use_errno=True).getpeereid
        function.argtypes = [ctypes.c_int, ctypes.POINTER(ctypes.c_uint32), ctypes.POINTER(ctypes.c_uint32)]
        function.restype = ctypes.c_int
        uid, gid = ctypes.c_uint32(), ctypes.c_uint32()
        if function(stream.fileno(), ctypes.byref(uid), ctypes.byref(gid)) != 0:
            number = ctypes.get_errno()
            raise OSError(number, os.strerror(number))
        return uid.value
    raise ValueError('Peer UID verification is unavailable on this platform')
