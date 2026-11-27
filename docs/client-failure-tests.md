# Deterministic client failure tests

```sh
./scripts/test-client-failures.sh
```

The script starts its own services and test clients in temporary
directories on Linux or macOS. It requires no running demo, language
model or additional dependencies. If the test runner is privileged, the
CLI scenarios run under the existing unprivileged test identity.

<a id="neue-fälle"></a>

## Transport cases

`crates/mynofy-cli/tests/failure_clients.rs` exercises real Unix and
WebSocket connections with the Rust client.

| Case | Expected behavior |
| --- | --- |
| Disconnect after polling | A poll without a cursor after reconnect returns the same active message with the same ID. A saved cursor resumes after that position. Consumers own durable processing progress. |
| Repeat a publish with the same request ID after reconnect | The owner assigns two different message IDs. Request IDs correlate responses. |
| Send malformed Protobuf or a payload over 4096 bytes | The request fails without publishing. An independent client can continue publishing and polling. A raw frame exercises the server's size validation directly. |
| Revoke a topic permission while a session is active | The old session closes and fresh publish or poll requests are denied. The authenticated producer retains access to its original content. Other authorized clients can continue. |
| Leave a WebSocket unauthenticated or an authenticated client idle | The service closes the connection after the configured timeout and accepts a subsequent client. |

Each wait has a test deadline. The timeout tests use short service
intervals to verify behavior.

## MCP cases

`crates/mynofy-cli/tests/mcp.rs` also checks:

- If the owner accepts a publish and disconnects before the response,
  MCP reports `outcome_unknown`. Reconnecting reveals one message and
  triggers no automatic republish.
- Cancellation and EOF end pending owner operations. Metadata remains
  available while application work is blocked.
- Real stdio MCP processes preserve client operations, ACL checks and
  poll behavior.
- Offline discovery and separation of supported MCP protocol modes.

The script runs the MCP and review follow-up integration suites. The core model and
mutation cases run through `scripts/test-core-functional.sh`.
Storage interruption cases run through `scripts/test-storage-crashes.sh`.
Human stop requests, mandates and business workflow decisions are
interpreted by the participating clients.
