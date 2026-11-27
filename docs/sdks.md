# Sprach-SDKs

Client support means connectable SDKs, not an agent runtime. languages share the existing
Protobuf and authentication protocol and common [Conformity vectors](../protocol/conformance/sdk-v1.json);
workflow, mandate, tickets and local specialist states remain a matter of use.

| Language | Support |
| --- | --- |
| Rust | Existing [Rust client crate](../crates/mynofy-client), reference for direct and tunneled access |
| Python | [standalone asyncio-SDK](../sdks/python/README.md), direct WSS and Unix access |
| Go | [Natives SDK](../sdks/go/README.md), direct WSS and Unix access |
| Java | [SDK for Java 17 and later](../sdks/java/README.md), direct WSS and Unix access |
| Bash | [Shell-Bibliothek](../sdks/bash/README.md) via Rust- CLI, direct and tunneled access |
| C | [Native C-ABI](../sdks/c/README.md) via Rust client, direct and tunneled connections |

<a id="gemeinsame-grundoperationen"></a>

## Common basic operations

Connection with Owner-/Client-identity verification, Publish, Poll with explicit cursor, own messages, permissions and catalog. Client-side key change receives the stable client ID.
SDKs must not derive any foreign rights from role claims or automatically repeat unclear
publishes.

The [Wire-Vektoren](../protocol/conformance/sdk-v1.json) check bytes, typed events, large `u64`
/ `i64`, Request-/Response-Fields and Auth-/Rotations-Transkripte. Rust generates and checks
them, Python, Go and Java read the same data.

<a id="verifizierter-python-umfang"></a>

## Verified python scope

Python uses three direct runtime dependencies: Official Protobuf Runtime, `websockets` and
`pyca/cryptography`. No CLI subprocesses in the SDK and no new Rust Registry dependencies.
Generated bindings are in the package; the compiler `grpcio-tools` is just a development tool.

Tests use self-launched Rust services: Unix and TLS 1.3/WSS, Owner/ Clientauth, ACL errors,
catalogs, bytes and events, including integer boundaries, Paging/Cursor and rotation
with subsequent rust- CLI Deterministic error peers check lost mutation results, timeout,
response binding, cancellation and closing during a request. `cargo test --workspace` to be
executed.

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -e './sdks/python[dev]'
./scripts/test-python-sdk.sh
```

For reproducible development dependencies
[requirements-test.lock](../sdks/python/requirements-test.lock) is available; it also contains
build and generator tools. The installed runtime package does not need it.

True native macOS testing follows with the planned CI. Peer tunnel, automatic
Polling-/state helper and admin access are not part of the first Python version. The existing
Rust client is still the full access for owner tunnels.

<a id="verifizierter-go-umfang"></a>

## Verified Go scope

The [Go-SDK](../sdks/go/README.md) supports the same direct basic operations and
client-initiated key change. Three direct dependencies: `coder/websocket`, official protobuf
runtime and `golang.org/x/sys` for peer UIDs. Cryptography and TLS use the Go standard library.
No CLI subprocesses and no Compiler-/generator dependencies at runtime; Go from 1.26.

`scripts/test-go-sdk.sh` performs native Rust service tests and deterministic
Rust-generated keys are readable in Go; Go-generated keys after rotation in Rust and optionally
Python. Darwin/ARM64 is crosscompiled; this does not replace a native macOS test. Peer tunnels,
admin access and Polling-/state helpers are excluded as in Python. First versions of all six
voice accesses are implemented. The CI in the private development repository configures their
common Linux-/macOS-checks; the native integration runs are also part of CI.

<a id="verifizierter-java-umfang"></a>

## Verified Java Scope

The [Java-SDK](../sdks/java/README.md) from Java 17 covers the direct basic operations and the
client-initiated key change. Two direct runtime dependencies: Java WebSocket and official
protobuf runtime, plus the transitive SLF4J - API. Kryptografie/TLS from the JDK; no additional
native or cryptography packages. JSON -Helps, protobuf- JSON and JUnit only in the test. Catalog
definition remains JSON -Bytes.

`scripts/test-java-sdk.sh` checks real Rust services, common wire vectors and
Error peers including thread interrupt, response loss and size limit. import Rust v2 keys
directly; check Java v2 export in Rust and optionally Python. PKCS #8 v1 also requires the
public key because the JDK does not offer direct seed→public keyAPI. Linux Unix transport tested
with peer check; macOS remains open for CI. Owner tunnel and admin as excluded at Python/Go.

<a id="verifizierter-bash-umfang"></a>

## Verified bash scope

Die [Bash-Bibliothek](../sdks/bash/README.md) Encapsulates the existing CLI with comfort
functions for Publish/Poll, own messages, rights, catalog, keygen and rotation. jq
exclusively for Strings/response validation; large numbers obtained as unchanged decimal texts.
Rust and cryptography dependencies.

Convenience calls each open a connection and check Request-ID/Antworttyp. broker error, CLI
failure, invalid responses and input errors have separate exit codes. Private temporary response
files and operation processes are cleaned up in the regular Ende/abfangbaren operation signal. A
JSONL passthrough keeps an CLI connection open if required; the raw Exit-/Antwortsemantik
remains. CLI does not provide separate Welcome-/Neustart-Metadaten.

`scripts/test-bash-sdk.sh` checks common Request-/Rust-Wire-Vektoren, real
Unix-/WSS-services, Auth/ACL, Bytes/Events, integer boundaries, paging, rotation, batch and
TLS errors. Doubles add quoting, response binding, exit codes and cancellation. Owner tunnel
flags checked; real multihop check still in the Rust-/CLI-Suite.

The Bash suite exists under Bash 3.2 and 5.2 on Linux; this does not replace a native macOS
test.

<a id="verifizierter-c-umfang"></a>

## Verified C perimeter

The [C-SDK](../sdks/c/README.md) is a separate workspace adapter crate over the Rust client.
Shared-/Static-Library with C11 header, without Server-/CLI-dependency or additional registry
packets. Typed Publish/Poll without JSON intermediate step, immutable response views instead of
additional payload copies, native U64/I64. keys and responses have explicit releases; the client
holds its own credentials.

A common I/O-Pool with two threads for all active C clients, sharing after the last client. One
operation per handle, Busy without queue and parallel close; no sharing of a handle during other
calls. C-Pointer conversion is the narrow exception to otherwise waiving your own unsafe code.

`scripts/test-c-sdk.sh`: Rust FFI unit tests plus separately compiled C program
real Unix-/WSS-Services, Auth/ACL, integer boundaries, Bytes/Events, cursor, response
lifetime, thread degradation and Key-Rotation/Rust-CLI-Interop. ASan/UBSan additionally check
the C-consumer; Rust remains uninstrumented. Owner tunnels use the existing Rust backend path
and its multihop integration tests. Native macOS- ABI check is configured in the CI in the
private development repository; first native run still open.
