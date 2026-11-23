# Broker client transport

Broker application commands now use one Protobuf frame per binary WebSocket
message. This applies to authentication, publish/poll, ACK/NACK, producer queries,
permissions, catalog retrieval and key rotation over WSS, local Unix sockets and
the inner owner connection of a proxy tunnel.

A JSON text application command closes the connection with WebSocket code 1003
before application dispatch. Direct WSS/Unix tests check the close code both
before authentication and after a valid Protobuf handshake. The owner-tunnel test
also checks rejection and absence of publication; a terminated owner channel may
surface as inner TLS EOF. Binary clients remain usable on fresh connections.

The Rust `exchange_json` text-wire function is removed. `exchange` still accepts
JSON-shaped values and translates them to Protobuf; `exchange_frame` remains the
typed API. CLI JSONL input/output, MCP JSON-RPC, JSON Data payloads and internal
JSON adapters are retained. The JSON schema describes the readable representation.
Historical in-process JSON benchmarks remain comparisons of adapters, not evidence
that the listener accepts JSON application messages.

The separate admin Unix socket uses its existing JSON protocol. Service discovery,
route queries and outer tunnel setup retain their distinct JSON control protocol.
They do not provide a JSON publish/poll/auth compatibility mode.

## Correlated rejection when parallel capacity is full (F1)

When a slow response holds a client's last parallel permit, another connection
of that identity can be rejected before command execution. That rejection now
includes the matching request ID, so existing SDKs report a normal
`resource_exhausted` broker error and keep the connection usable. It is no longer
reported as an ambiguous publish transport failure.

Only this rejection path uses a schema-generated metadata projection. All known
body tags and their command/response classification come from the same generator
as the application schema. The projection uses prost and the same field/oneof
merge semantics, with byte views for IDs; it skips payloads and other fields.
The existing WebSocket byte bound is checked first, then the final ID must be
nonempty valid UTF-8 and at most 128 bytes before being copied to the error frame.
Oversized IDs never become correspondingly sized String allocations. Payload
contents are not decoded or validated by this projection: the command is being
rejected, not accepted. Unknown fields use the decoder's normal framing and
recursion rules. Malformed/unidentifiable metadata closes without execution.

The successful request path is unchanged. A client that exhausts its rate bucket
still receives the existing pre-decode `rate_limited` response with backoff; an
invalidated identity is closed. No wire field, error code, dependency, SDK-wide
anonymous-error exception or automatic publish retry is added.

Tests check byte/UTF-8 boundaries, borrowed ID storage, skipped invalid payloads,
field order, repeated fields/oneofs, response-direction rejection, malformed and
truncated framing, unknown groups/recursion and 4,096 seeded mutations with
comparison against full decoding where applicable. A real Unix integration holds
a response, rejects a publish with its ID, checks that invalid metadata closes
without publication, drains the response and reuses the same healthy connection.
The Python SDK repeats the capacity case over Unix and WSS, confirming ordinary
BrokerError handling, unchanged message count, and reuse after draining.

## Earlier binary-only transport validation

Linux checks passed:

- Workspace: 265 passing Rust tests, 1 expected ignored storage worker.
- Clippy for all workspace targets with warnings denied; formatting.
- Explicit rejection tests for Unix/WSS before and after authentication and for
  the inner owner tunnel, including absence of JSON-publish side effects.
- Binary Ping/Pong, malformed/oversized Protobuf, TLS and service-link suites.
- Python, Go, Java, Bash and C SDK suites; both English code-review demo tests.
- Rust 1.88.0 all-target MSRV check, protocol without default features, 23 CI/export
  regression tests, actionlint and generated binding consistency.

Raw logs and source hashes remain private under
`benchmarks/binary-client-transport-20261115/`. Native macOS execution and new
performance measurements were not performed in this Linux container. The F1 implementation and its verification are recorded below.

## F1 verification

Verified locally on Linux:

- Workspace tests: 273 passed, one intentionally ignored crash-worker entry.
- Seven metadata-projection tests cover bounds, UTF-8, field merging, unknown groups, truncation and 4,096 seeded mutations.
- Real capacity tests verify correlated rejection, absence of publish side effects and connection reuse. Python exercises both Unix sockets and WSS.
- All Python, Go, Java, Bash and C SDK suites passed.
- Workspace Clippy with warnings denied, formatting, minimal protocol features and the Rust 1.88 all-target workspace check passed.
- All 23 CI/export helper tests, actionlint, generated-file consistency and both English example tests passed.

No native macOS run or new performance measurement was performed for this change. The admitted-request path is unchanged; there are no new dependencies, wire fields or production SDK changes.
