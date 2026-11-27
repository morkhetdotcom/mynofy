# mynofy C-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Small native C- ABI via the existing Rust client library: long-lasting connections, direct
WSS-/Unix- and owner tunnels, typed Bytes/Events, Publish/Poll, own messages, rights, catalog
and key change. No CLI subprocesses, no additional Protobuf-/TLS-/Kryptoanbieter and no new
registry dependency. Header: [mynofy.h](include/mynofy.h).

The ABI is, like the protocol, a mutable prototype. Header and library must come from the same
version. Config size and ABI version are checked.

<a id="bauen-und-einbinden"></a>

## Building and integrating

```sh
cargo build --locked --release -p mynofy-client-c
```

Erzeugt `target/release/libmynofy_c.so` (Linux), `.dylib` (macOS) and each a static `.a` The
consuming C application requires only library and header compilation at runtime; no Rust
toolchain, Python, CLI or Protobuf compiler. the library contains the existing
Rust-/AWS-LC-/Rustls-/Protobuf-Clientstack.

Under Linux, build a complete [C11 -Example](examples/local.c):

```sh
cc -std=c11 -Wall -Wextra -Werror -pedantic -I sdks/c/include \
  sdks/c/examples/local.c -L target/release -lmynofy_c \
  -Wl,-rpath,"$PWD/target/release" -o /tmp/mynofy-c-example
/tmp/mynofy-c-example SOCKET CLIENT.pk8 local local/events
```

CMake with pre-built library:

```sh
cmake -S sdks/c -B target/c-sdk-example \
  -DMYNOFY_LIBRARY="$PWD/target/release/libmynofy_c.so"
cmake --build target/c-sdk-example
```

On macOS the `.dylib` -Insert path; the library uses the portable install name
`@rpath/libmynofy_c.dylib` The CMake import `mynofy::client` contains headers and system link
dependencies and also accepts the `.a` When manually linking static in Linux, use the system
libraries reported by the compiler: `-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`.

<a id="verbindung-und-publish"></a>

## Connection and Publish

Public key and topic rights must be registered with the owner. Private keys of `mynofy keygen`
are directly importable; `mynofy_key_parse` accepts the raw PKCS #8 bytes. API does not generate
or read key files itself.

```c
#include "mynofy.h"

/* The application securely loaded der and der_len. */
mynofy_key *key = NULL;
mynofy_client *client = NULL;
mynofy_response *response = NULL;
mynofy_error error;
int32_t status = mynofy_key_parse(der, der_len, &key, &error);
/* Check each call status and release resources on error. */

mynofy_config config = MYNOFY_CONFIG_INIT;
config.socket_path = "/privater/pfad/mynofy.sock";
config.owner = "local";
config.key = key;
status = mynofy_connect(&config, &client, &error);

const uint8_t bytes[] = {0, 1, 255};
status = mynofy_publish_bytes(client, "local/events", bytes, sizeof bytes,
                            "application/octet-stream", NULL, &response, &error);
/* Publish result: message ID and owner-selected expiration. */
mynofy_str id = mynofy_response_message_id(response);
/* id.data is borrowed and has no NUL terminator. Copy it before freeing the response. */
mynofy_response_free(response);
mynofy_client_free(client);
mynofy_key_free(key);
```

The complete example checks for errors and cleans up all handles. For WSS instead of
`socket_path` set the fields `url="wss://HOST:PORT"` and `ca_file`. TLS 1.3,
CA-/Hostname-verification and Keypair-Auth, the Rust client takes over its protected path, socket and
kernel peer check. No option to turn off these checks, no Klartext-/Proxy-Umgebungsfallback.

Owner tunnel: `via_topic`, `owner_ca_file`, optional `owner_name`; `owner` is the identity of
the target owner. `ca_file` checks the proxy for external WSS, the owner-CA checks the internal
TLS endpoint. Client-Auth and ACL end with the owner.

`mynofy_client_info` provides owner, stable client ID and U32 restart counter.
After a known restart, deliberately read without stored poll cursor and compare it with your own
state. SDK does not automatically manage any progress.

<a id="events-und-antworten"></a>

## Events and Answers

```c
mynofy_event event = {0};
event.catalog = 1; event.catalog_version = 1;
event.category = 12; event.action = 42; event.status = 2;
event.parameter_count = 2;
event.parameters[0].type = MYNOFY_U64;
event.parameters[0].value.u64_value = UINT64_MAX;
event.parameters[1].type = MYNOFY_I64;
event.parameters[1].value.i64_value = INT64_MIN;
```

Take IDs and parameter meaning from catalog/Anwendungsvereinbarung. Initialize whole structure
with `{0}`, then set IDs, number and active Union members. At most three parameters:
U32/U64/I32/I64/F32/F64/BOOL; BOOL exactly 0/1. Not finite floats and unknown catalog IDs are
rejected locally. `mynofy_publish_event` does not require a JSON intermediate step. Raw bytes
maximum 4096, including NUL. `expires` is optional RFC3339 time text, otherwise topic policy
applies. All integers remain native `uint64_t` / `int64_t` without rounding.

`mynofy_poll(client, topic, after, limit, &response, &error)` immediately gives a
limited page back; `after=NULL` reads the active stock. Limit 1..100. Access entries with
`mynofy_response_message_count` and `mynofy_response_message_at`; these contain payload and
expiration. fetch the next page immediately at `has_more`.

Producer lists contain summaries without payload; use `mynofy_get_message` for this. Keep the
filter and limit for paging at `mynofy_list_messages`. Message views have no delivery-ID or
confirmation-counter fields.

`mynofy_get_permissions` delivers actions, owners and ACL revision of the
`mynofy_get_catalog` provides the catalog definition as uninterpreted JSON bytes; a JSON parser
remains a matter of application.

<a id="besitz-und-parallelität"></a>

## Possession and parallelism

- Keys, clients and responses are opaque handles, releasing every successful result exactly once
  with the associated `*_free` function, never with C- `free`.
- Inputs are NUL -terminated UTF -8 strings or readable data buffers with length. Optional
  strings are `NULL`; empty mandatory names are invalid. `NULL` for bytes only initialize
  completely at length 0. Config/Event.
- Outputs must be valid and writable. Handle outputs start as `NULL`; release existing responses
  before reuse. Error output is optional.
- String-/Byte-Views are immutable and **not NUL -terminated. They are valid until the release
  of their own handle. Message-Views/Parameterwerte are stack copies; their data pointers borrow
  the response memory. A response remains readable even after the release of the client. No
  further payload copy during reading.
- The client copies its credentials on the Connect; the original key may then be released.
  Private SDK key buffers are zeroed on drop; own exported Kopien/Dateien remain the
  responsibility of the application.
- One operation per client. Simultaneous attempts yield `MYNOFY_BUSY` without command sequence.
  Different clients can work from several C threads.
- `mynofy_client_close` may run in parallel: permanent cancellation and closure of the
  transport. `client_free` call only when all calls have returned. Keys/Responses also release
  only after the end of all reading accesses.
- Synchron/blockierend. All active C clients share a I/O-Runtime It ends after sharing the last
  client; it doesn't require responses and keys. Runtime/Workergruppe.

The necessary `unsafe` code lies exclusively in this C adapter crate.
Null-/sizes-/UTF-8-/Enum-checks does not replace the responsibility for valid pointers,
lifetimes and exactly one release. Rust panics are intercepted at the ABI boundary; no Rust
exceptions are handled by a C stack.

<a id="fehler-abbruch-und-rotation"></a>

## Error, termination and rotation

Status is `MYNOFY_OK` or a code from `enum mynofy_status`. `mynofy_error` contains code, limited
NUL-terminated diagnosis, broker code and optional `retry_after_ms`.

Broker rejection keeps the connection usable. Transport-/protocol errors, Timeout or close
discard it. `outcome_unknown` indicates that Publish/Rotation could already be effective. Never
resend automatically. Reconcile familiar messages and local state, explicitly reconnect. There
are no automatic ACKs, reconnects, poll loops, state files or admin functions.

`config.timeout_ms` Limited connection setup including authentication
and one operation each, standard 10000 ms. The complete key change also has this budget; in
addition, the deadlines of the owner and the fixed Phasen-/Austauschlimits of the Rust transport
apply.

Before `mynofy_rotate` create the new key with `mynofy_key_generate` and save it safely via
`mynofy_key_pkcs8`, e.g. exclusive file with mode 0600. Success closes the old session.
Reconnect with new key and original `config.client_id`. Keep both keys if commit unclear. No
filewriting and no automatic credentials replacement with the API.

## Tests

As a normal user with Rust/Cargo, C11 compiler, Python standard library and OpenSSL:

```sh
./scripts/test-c-sdk.sh
cargo clippy --locked -p mynofy-client-c --all-targets -- -D warnings
```

Rust unit tests check FFI input, Verlust/Timeout/Busy/Close, response binding, structured rate
limits, panic limit, common event wire vectors. A separately compiled C program checks real
Unix-/WSS-services, ACL, bytes and events, paging, views for client sharing and rotation with
Rust CLI login. Linux also checks that no SDK-I/O-Threads remains after client sharing. The
stand-alone C example is compiled and executed.

The stdlib test fixture is shared with the Bash suite; neither Bash SDK nor CLI are runtime
dependencies of the C application. TLS-/Mehrhop-checks of the shared Rust transport remain in
its existing integration suite. Native macOS-ABI and runtime verification follows in the CI. ASan/UBSan
on the C test client instrument this consumer, not the prebuilt Rust library.

## Owner-authenticated producer identity

`mynofy_response_message_producer_id(response, index)` returns the immutable stable
client identity stamped by the owner for poll and get-message results. The UTF-8 view is borrowed from the response, is not NUL-terminated, and remains valid until response-free. Invalid indices, null responses, and producer-list summaries yield an empty view. This accessor does not change the existing message struct layout. Match application actor IDs to this value rather than trusting payload claims.

Rotation rejections preserve the owner's code, detail and backoff in `mynofy_error` and return `MYNOFY_BROKER` with `outcome_unknown = 0`, including rejections during confirmation and commit. The connection remains available for an explicit next operation. A lost or invalid commit response remains an unknown outcome; keep both keys and verify credentials through an explicit reconnect. No automatic retry occurs. See [key rotation guide](../../docs/key-rotation.md).

For cursor continuity, compare the value from `mynofy_client_boot_id` between authenticated sessions. This borrowed string is empty for legacy owners and is valid while the handle lives.
