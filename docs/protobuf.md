<a id="protobuf-und-typisierte-payloads"></a>

# Protobuf and Typed Payloads

Release **v0.1.0 is the first compatibility baseline**. Earlier development and demo formats are not supported releases and require no migration support. Within protocol version 1, preserve existing field numbers, meanings and required fields. New optional fields must preserve behavior when absent; retired tags stay reserved. Incompatible changes require a new protocol version and an explicit upgrade plan. The listener rejects unsupported frame versions.

<a id="verfügbare-schnittstelle"></a>

## Available interface

Client and CLI use only binary protobuf messages via the same secured WebSocket-/Unix-Transport.
A WebSocket message contains exactly one prototype frame, and no additional length prefix is
required within the WebSocket message. JSON Application commands as text frames are rejected
before and after authentication with WebSocket code 1003. JSON control frames for Peers/Tunnel
They are their own interfaces.

The CLI continues to read and write JSONL for people and terminal tools, but it converts at the
boundary between the readable display and Protobuf. For direct access without JSON conversion,
the SDK`exchange_frame` with `mynofy_protocol::pb::Frame`. `exchange` is the previous JSON
-shaped convenience API with Protobuf on the network; `exchange_json` has been removed; a JSON
-text wire mode for client commands no longer exists.

All existing local client commands have protobuf variants: Auth, Publish/Poll, Producer queries,
permissions, catalog retrieval and rotation. Peer/proxy control messages use a separate
protocol; see [networking](networking.md). Protocol version 1 uses release v0.1.0 as its
compatibility baseline.

<a id="schema-und-generierung"></a>

## Scheme and generation

[generate.py](../protocol/protobuf/generate.py) contains the field definitions and generates the [portable proto2-Schema](../protocol/protobuf/mynofy.proto), prost-Derives, JSON -Adapters and the static catalog enums/membership checks. Existing field numbers must not be reinterpreted or reused. There is no descriptor loader required at runtime and no build dependency on `protoc` / `prost-build`.

```sh
python3 protocol/protobuf/generate.py
cargo fmt --all -- --check
cargo test --locked -p mynofy-protocol
```

The generator is development tooling and requires Python/Rustfmt; normal cargo builds use the
checked-in files. The Rust names `pb::Result` and `payload::Body` are protocol types, not Rust
error handling.

The implementation uses the official [prost-Upstream](https://github.com/tokio-rs/prost) and
[Bytes](https://github.com/tokio-rs/bytes). `bytes` was already transitive. The additional
runtime library is prost; derive brings necessary build and macro dependencies. There are no local
forks or git patches.

## Zwei Payload-Varianten

Free bytes consist of bytes and optional `content_type` (1–128 UTF -8 bytes). Hard cap: **4096
actual data bytes**, possibly further restricted by `limits.max_payload_bytes`. There is no
automatic fragmentation, shortening or large pre-reserved payload per message. In RAM, the core
bytes holds as divided, immutable `Bytes`; poll copies do not have to copy their contents again.

Lesbare representation:

```json
{"version":1,"type":"publish","id":"p1","topic":"local/events","payload":{"kind":"bytes","content_type":"text/plain","data":"SGFsbG8="}}
```

`data` is the standard canonical base64 only in the JSON representation, and the protobuf wire contains the actual bytes, and the example contains five useful bytes (`Hallo`).

Typed events contain numerical catalog-ID/-version, category, action, status and a maximum of
three primitive parameters with a total of 24 pure value bytes. Catalog ID 1 denotes `mynofy`,
version 1. unknown Versionen/IDs, missing parameter values and non-finite floats are rejected.
The basic types are U32/U64/I32/I64/F32/F64/Bool. No strings or reloaded catalogs within an
event.

```json
{"version":1,"type":"publish","id":"p2","topic":"workflow/4711/communication","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":40,"status":7,"parameters":[{"u64":4711}]}}
```

This means `(JOB, EXECUTE, PENDING)` with a U64 parameter. The specific meaning of this
parameter belongs to the agreed profile. The core checks known identifiers and sizes, no
technical transitions or roles within a payload. The
[Enums](../crates/mynofy-protocol/src/catalog_values.rs) and numerical checks are generated from
the catalog; no JSON catalog scan per event.

A payload is either event or bytes. Parameters plus free bytes in the same payload are not a
third implicit variant. Required context must be known in the Profil/Topic or be in a single
free payload.

<a id="kompatibilität-und-migration"></a>

## Compatibility and migration

- Older arbitrary JSON payloads remain readable as a convenience display. They are stored
  internally as UTF -8- JSON bytes; their complete JSON serialization must fit in 4096 bytes. A
  string with 4096 characters additionally requires its JSON quotes and thus no longer fits.
- The payload objects with `kind: "event"` or `kind: "bytes"` are reserved for the new variants.
  The internal content type `application/vnd.mynofy.legacy+json` serves the JSON compatibility
  adapter; it checks UTF-8/JSON and must not be used as an opaque binary type.
- Existing service configurations with `max_payload_bytes: 65536` must be upgraded to a maximum
  of 4096. New scenario templates already set 4096. Existing local `.local/*` files are not
  changed unasked. Restart the server after update.
- Response sizes and transport sizes still have their own limits; a poll page or catalog
  response may be larger than a single payload.
- Protobuf ignores unknown fields according to the decoder used; oneof-/Scalar-retries
  follows its merging rules. The readable JSON input of the CLI checks duplicate keys and
  unknown command fields before translation. The broker checks version, direction, identity,
  ACL, limits and state in addition to the Protobuf codec.
- Auth key fields/signatures and RFC3339 times retain their existing text representation in the
  typed control messages for the time being, which does not change signature transcripts or
  rotation bindings. The rare catalog retrieval transports the independently versioned JSON
  definition as bytes in a prototype catalog response.

<a id="speicher-und-messung"></a>

## Memory and measurement

The news is in [kompakten RAM-Format](binary-message-layout.md) with numerical references, time
values and embedded event parameters instead of wire messages, or JSON -Object trees. Protobuf-
`StoredMessage` Publish and Poll have a direct protobuf path. Auth and rotation continue to use
JSON -conversion to its internal adapter; Publish/Poll/Get/List/Permissions/catalog They have
native protobuf paths. JSON -free server to derive. Configuration supported YAML/JSON; CLI
-Representation and catalog definition continue to use JSON.

The storage and transport guides separates the same current core with two encodings from the
additional memory conversion compared to the older implementation. Netzwerk/TLS-Durchsatz was
thus not measured.
