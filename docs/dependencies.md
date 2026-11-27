<a id="abhängigkeiten-des-ersten-rust-prototyps"></a>

# Dependencies of the first Rust prototype

The workspace declares fifteen different direct external dependencies, plus internal path
dependencies between its four crates. There are no additional dev dependencies. There are no Git
dependencies, `[patch]` overrides, or project-specific forks. `Cargo.lock` keeps the resolution
versions and registry checksums.

| Crate | Utilisation | Upstream |
| --- | --- | --- |
| `tokio` | Async-Runtime, TCP, timer, limited parallelism. | [tokio-rs/tokio](https://github.com/tokio-rs/tokio) |
| `tokio-rustls` | TLS 1.3 with Rustls; PEM processing via its existing PKI types. | [rustls/tokio-rustls](https://github.com/rustls/tokio-rustls) |
| `tokio-tungstenite` | WebSocket handshake and frames, without further web framework. | [snapview/tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) |
| `futures-util` | Required Stream-/Sink-extensions; Features limited to `std` and `sink`. | [rust-lang/futures-rs](https://github.com/rust-lang/futures-rs) |
| `serde` | Typed configuration and protocol fields. | [serde-rs/serde](https://github.com/serde-rs/serde) |
| `serde_json` | JSON frames and CFG; duplicate keys are also explicitly rejected. | [serde-rs/json](https://github.com/serde-rs/json) |
| `saphyr-parser` | YAML configuration: pure event parser without default features; no Alias-/Tag-processing by the application. | [saphyr-rs/saphyr](https://github.com/saphyr-rs/saphyr) |
| `aws-lc-rs` | The same official provider for Rustls and Ed25519, SHA -256, HMAC, CSPRNG, PBKDF2 and AES - GCM. | [aws/aws-lc-rs](https://github.com/aws/aws-lc-rs) |
| `base64` | Canonical Token-/key-/Cursor-Encoding. | [marshallpierce/rust-base64](https://github.com/marshallpierce/rust-base64) |
| `time` | RFC -3339 validation and formatting. | [time-rs/time](https://github.com/time-rs/time) |
| `prost` | Protobuf Messages and Derives; no runtime descriptor framework. | [tokio-rs/prost](https://github.com/tokio-rs/prost) |
| `bytes` | Unchangeable shared payloads, previously already transitively present. | [tokio-rs/bytes](https://github.com/tokio-rs/bytes) |
| `zeroize` | Discard sensitive key/secret buffers. | [RustCrypto/utils](https://github.com/RustCrypto/utils) |
| `rustix` | Linux: secure UID/GID-, group, capability and `no_new_privs` calls in the CLI bootstrap; features limited per use: `std` / `process` and `thread` (CLI) or `fs` (server). | [bytecodealliance/rustix](https://github.com/bytecodealliance/rustix) |
| `nix` | Other Unix platforms: Identity verification (macOS denied root) and available POSIX calls; only `user`. | [nix-rust/nix](https://github.com/nix-rust/nix) |

Tokio, Rustls, Tungstenite and the cryptography provider bring necessary transitive
dependencies. The number of direct dependencies does not denote the total number of transitive
crates. Versions and common features are centralized under `[workspace.dependencies]` in the
root manifesto. The provider requires native build tools; OpenSSL is not a TLS runtime
dependency of the service. No second Signatur-/AEAD-Stack is built in.

The archived wrapper `rustls-pemfile` has been removed before completion; its upstream [Refers
to the PKI - PEM APIs](https://github.com/rustls/pemfile/releases). The selection has been
vetted against upstream repositories and current releases. An ongoing maintenance commitment
cannot replace this vette; before updates, changes, security messages and the active dependency
tree must be rechecked.

```sh
cargo tree --locked --workspace --depth 1
cargo tree --locked --workspace --duplicates
cargo metadata --locked --format-version 1
```

The active tree currently contains two versions of the small transitive `untrusted` helpcrate
from the provider and WebPKI. This comes from the official upstreams; they are not additional
project forks.

The [Rust client crate](../crates/mynofy-client) prevents server dependency of the client SDK.
The protocol crate can be built without an optional `crypto` feature, regardless of network
runtime and cryptography provider. The workspace division does not introduce new registry
dependencies.

Protobuf adds prost 0.14.5 from the official upstream. `prost-derive` brings necessary Proc
macro dependencies (among others anyhow/itertools/syn). No prost-build, vendored protoc or build
generator as cargo dependency. Python/Rustfmt regenerate only when developing the checked-in
types; details in [Protobuf guide](protobuf.md).

For the early [Rechteabbau](privileges.md) The official upstreams of rustix 1.1.6 (Bytecode
Alliance) and nix 0.31.3 were reviewed. CLI selects their bindings platform-specific. The server
also uses rustix under Unix for secure no-follow file accesses, atomic renaming and filelocks of
the security journal; no additional registry crates are added. Client and protocol remain free
of these OS bindings. Rustix delivers secure under Linux Capability-/prctl- and explicit thread
ID APIs; nothing provides identity verification on macOS; since its group APIs are excluded
there, mynofy denies privileged starts there. `forbid(unsafe_code)` It's not a fork or its own
OS. FFI -Binding. The separate C-- SDK -Adapter is the explicitly required C- ABI -Ausnahme.

YAML uses saphyr-parser 0.3.4 from the official Saphyr project (package MSRV 1.85, compatible
with project MSRV 1.88). Upstream, release and manifest were checked before selection.
Production dependencies are arraydeque and thiserror; thiserror was already transitive. No C-
YAML library, no additional serde YAML wrapper and no git patches. The server alone receives the
dependencies; the client and protocol remain YAML -free. The archived serde_yaml is not
introduced. The deliberately restricted event connection and export rules are in
[YAML-Leitfaden](yaml-config.md).

Content encryption also uses the existing official AWS LC provider directly in the server,
without additional registry dependency or second crypto stack. The CLI additionally uses
rustix/termios for hidden stdin input under Unix; native macOS validation is pending.
Parameter/Referenzen in [Content-Leitfaden](content-storage.md).

## C-SDK

The additional Workspace Crate `mynofy-client-c` only uses existing Workspace dependencies:
Client, Tokio/Rustls/Tungstenite and Zeroize. No new registry version and no second
Krypto-/Protobuf-Anbieter. Tokio gets the already available feature `rt-multi-thread`; two
common Workers for active C clients instead of one Runtime per connection. Erstes/letztes Handle
control the Runtime lifecycle. `prost` / `serde_json` / `futures-util` are direct test aids, no
new C Runtime API. Pointer and Union conversion remains in the C adapter; Core/Transport and the
previous Sicherheits-/OS-Bindings remain unchanged.
