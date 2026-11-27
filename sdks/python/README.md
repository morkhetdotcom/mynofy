# mynofy Python-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Stand-alone `asyncio` client from Python 3.11, without CLI subprocess or server core dependency.
Direct WSS connections with TLS-/Hostname-verification and local Unix sockets with
path and peer UID verification. wire format: existing prototype v1. API is, like the protocol, still a
variable prototype.

<a id="installation-aus-dem-repository"></a>

## Installation from the repository

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install ./sdks/python
```

Three direct runtime dependencies: official
[Protobuf-Runtime](https://pypi.org/project/protobuf/),
[websockets](https://pypi.org/project/websockets/) and
[pyca/cryptography](https://pypi.org/project/cryptography/). No fork and no second crypto
provider. `grpcio-tools` is used exclusively for the generation of bindings, not the running
SDK. The `.proto` source is in the repository below `protocol/protobuf/mynofy.proto`; generated
Python types are supplied.

<a id="unix-socket-bytes-senden-und-lesen"></a>

## Unix socket: Send and read bytes

The public key must be registered with the owner; a login does not register a client and grants
no rights; keys from `mynofy keygen` are compatible.

```python
import asyncio
from pathlib import Path
from mynofy import Client, Key

async def main():
    key = Key(Path('.local/unix/client.pk8').read_bytes())
    async with await Client.connect_unix(
        '.local/unix/run/mynofy.sock', owner='local', key=key,
    ) as client:
        print(client.info)  # Owner, stable client ID, restart counter and optional boot ID
        sent = await client.publish(
            'local/events', 'Hallo'.encode('utf-8'), content_type='text/plain',
        )
        page = await client.poll('local/events', limit=20)
        for item in page.items:
            print(item.message.id, item.message.payload)
            # After Verstehen/Zuordnen; this is not a professional degree:
        # Store locally after processing or permanent own takeover:
        cursor = page.next_cursor
        following = await client.poll('local/events', after=cursor, limit=20)
        print(sent.message_id, following.has_more)

asyncio.run(main())
```

Pick up more pages immediately at `has_more`. Poll does not start a background loop. Missing
cursor reads the active stock again. Discard cursors saved by current owners after a known
service restart based on `client.info.boot_id` and compare them with local work status; no
automatic repetition of specialist actions. SDK does not store cursor, tickets or mandates and
does not perform professional confirmation. Poll reads without recording reader state. Losing
its response leaves client-local progress unchanged.

## WSS

```python
client = await Client.connect_wss(
    'wss://localhost:8080', owner='local', key=key,
    ca_file='.local/demo/ca.crt', timeout=10,
)
# Use and then expect client.close(), or async with client.
```

Only `wss://`, at least TLS 1.3, CA and hostname check. `ca_file=None` uses the operating system
trustroots. No option to turn off the check and no adoption of
HTTP-/SOCKS-Proxy-Umgebungsvariablen. Unix used under Linux `SO_PEERCRED`, under macOS
`getpeereid`; native macOS verification follows in the CI. Unsupported peer UID query leads to
cancellation, not weaker check.

<a id="ereignisse-und-weitere-operationen"></a>

## Events and other operations

```python
from datetime import datetime, timedelta, timezone
from mynofy import Event, Parameter

catalog = await client.get_catalog()
# IDs and technical parameter meaning from catalog/application protocol agreement:
event = Event(catalog=1, catalog_version=1, category=12, action=42, status=2,
              parameters=[Parameter(u64=2**64 - 1)])
await client.publish('local/events', event,
                     expires=datetime.now(timezone.utc) + timedelta(seconds=60))
rights = await client.get_permissions('local/events')
own = await client.list_messages(topic='local/events', limit=20)
# Keep the paging filter and limit:
if own.HasField('next_cursor'):
    own_next = await client.list_messages(topic='local/events', limit=20,
                                          cursor=own.next_cursor)
status = await client.get_message(sent.message_id)
```

Bytes are explicitly encoded, maximum 4096 bytes; events contain maximum three
numerische/boolesche Parameters. No implicit String-/JSON-Konvertierung. Receive Python Integer
and Protobuf `u64` / `i64` No floating point route. Returns are the supplied prototype buf types
() `mynofy.pb` ); `get_catalog` Then it will be tested. JSON -Dictionary. Optional fields with
`HasField` unterscheiden.

<a id="fehler-parallelität-und-schlüsselwechsel"></a>

## Error, parallelism and key change

- `BrokerError`: Rejected request with `code`, `detail`, optional `retry_after_ms` No automatic
  repetition or waiting or backoff strategy.
- `TransportError`: Connection to I/O-/protocol errors In mutation, it can be rejected.
  `outcome_unknown=True` gelten: Publish/Rotation do not send again blindly, but compare with
  the owner and based on their own condition.
- Cancellation during a request also discards the connection, and even at `CancelledError` a
  mutation may have already been assumed.
- One operation per client. Simultaneous call results in `BusyError`, not an unlimited queue.
  Use separate connections for parallelism.
- Connection is not automatically re-established. Owner-/Client-identity and restart counter
  check for explicit reconnection.

Client-side change with `await client.rotate(new_key)`: save new key **before** yourself
securely. After success, the old session is closed; reconnect with new key and unchanged
`client_id=old_client_id`. Keep both keys and check the new access if the commit is unclear. SDK
does not write key files.

`Key.generate()` generates Ed25519, `public_key`, `token_id`, `client_id` deliver the
Registration data. `private_bytes()` exports the Rust-compatible PKCS #8-v2 representation;
import accepts standard v1 and the verified Rust-v2 form. Python does not guarantee complete
erasure of all key copies.

<a id="grenzen"></a>

## Boundaries

Kein Peer-/Owner-Tunnelzugang in this first version of Python; Rust/CLI They already have
control over him. API, topic system, agent logic, status files or polling daemons in SDK The
service remains the sole ACL -Entscheider.

<a id="entwicklung-und-konformität"></a>

## Development and conformity

```sh
python -m pip install -e './sdks/python[dev]'
python sdks/python/generate.py
./scripts/test-python-sdk.sh
cargo test --locked -p mynofy-protocol --features crypto sdk_conformance
```

The tests require the built Rust binary, OpenSSL CLI and a normal user. They start their own
services in temporary directories, no existing demo is changed. WSS/Unix, Auth/ACL, catalog,
Bytes/Events, large integers, cursors, confirmations, Python↔Rust key formats and rotation are
checked in real time; error peers check timeout, wrong response, cancellation and lost results.

`protocol/conformance/sdk-v1.json` contains the same Wire-/Auth-/Rotationstranskripte
for Rust and Python and later SDKs. Regeneration of the Vectors:

```sh
cargo run --quiet --locked -p mynofy-protocol --features crypto --example sdk_vectors > protocol/conformance/sdk-v1.json
```

## Owner-authenticated producer identity

Read `item.message.producer_id` for the stable client identity stamped by the owner. It is immutable, survives authorized key rotation and storage restore, and is separate from any sender or actor claim inside the payload. Match workflow roles to this value. This is an assertion over the protected owner connection, not a portable cryptographic message signature.
