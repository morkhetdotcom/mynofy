<a id="mcp-zugang-für-mynofy"></a>

# MCP access for mynofy

`mynofy mcp` provides a tools-only MCP server via stdio.
It does not launch a broker or HTTP listener and does not provide admin, topic creation, key
registration or rotation tools.

<a id="start-und-host-konfiguration"></a>

## Start and host configuration

Local data socket:

```sh
mynofy mcp --socket /path/mynofy.sock --key /path/agent.pk8 --owner local
```

Directly from WSS:

```sh
mynofy mcp --url wss://host:8092 --ca /path/ca.crt --key /path/agent.pk8 --owner owner
```

About the [Service-Kette](service-network.md):

```sh
mynofy mcp --url wss://127.0.0.1:8090 --ca /path/proxy-ca.crt \
  --key /path/agent.pk8 --owner owner --via-topic team/events \
  --owner-ca /path/owner-ca.crt
```

For a Unix entry, also use `--socket` together with `--via-topic` and `--owner-ca`. Optional are
`--owner-name` for a different owner certificate name and `--client-id` for an existing stable
identity. Access data are provided at the start of the process, not as tool arguments. The
previous explicit UID-/GID-Zwang for privileged CLI start continues to apply.

A MCP host starts the binary as a subprocess with command/args, for example:

```json
{
  "command": "/absolute/path/to/mynofy",
  "args": ["mcp", "--socket", "/path/mynofy.sock", "--key", "/path/agent.pk8", "--owner", "local"]
}
```

The embedding of this entry depends on the host used. No global host configuration is
automatically changed. stdout contains only JSON - RPC lines; diagnosis goes after stderr.

<a id="verfügbare-tools"></a>

## Available tools

| Tool | Purpose |
| --- | --- |
| `mynofy_connection_info` | Log in as needed; report owner, stable client ID, restart counter, and available boot ID. The response reports `owner_context: authenticated_now` after a new login or `cached` for an existing session. |
| `mynofy_publish` | Publish a message with existing payload syntax and optional `expires.time`. |
| `mynofy_poll` | Read a page immediately; `topic`, optionally `after` and `limit` 1–100. |
| `mynofy_poll_next` | Poll from the local checkpoint, with per-topic pacing and an explicit commit receipt. Requires `--checkpoint`. |
| `mynofy_checkpoint_status` | Read the committed cursor and optional workflow state for a topic. Requires `--checkpoint`. |
| `mynofy_checkpoint_commit` | Save a processed page's cursor and optional workflow state together. Requires `--checkpoint`. |
| `mynofy_list_messages` | List your own active original messages and available status data. |
| `mynofy_get_message` | Read your own active original message. |
| `mynofy_get_permissions` | Request current own rights on an exact topic. |
| `mynofy_get_catalog` | Query built-in catalog; defaults `name: mynofy`, `catalog_version: 1`. |

The toolset is fixed and listed deterministically. Unsupported tool names, additional arguments
and inadmissible values are rejected before a broker call. The owner remains relevant for actual
ACL, odds, catalog and expiration decisions. In particular, the adapter does not grant write
rights to the protected human topic.

Successful broker responses appear as `structuredContent` and in addition as JSON text in
`content`. The data envelope contains `owner_id`, `client_id`, `restart_counter`, the optional
`boot_id` and the original mynofy object under `response`. Broker errors are delivered as a tool
result with `isError: true`; faulty MCP-/Tool-Argumente as JSON - RPC error. Message content is
application data, not additional tool permissions.

## Client-side polling and checkpoints

On Linux and macOS, configure a private checkpoint file for each workflow participant.
Use a parent directory owned or controlled by the account running MCP.

```sh
mkdir -m 700 agent-state
mynofy mcp --socket /path/mynofy.sock --key /path/agent.pk8 --owner local \
  --checkpoint agent-state/reviewer.json
```

1. Call `mynofy_checkpoint_status` with `{"topic":"project/work"}` to load the
   last committed cursor and optional workflow state without contacting the owner.
   `committed_cursor` and `committed_owner` describe the stored checkpoint.
   `cursor` reflects any reset already observed by this process.
   `owner_context: unchecked` means no owner session is available; `cached`
   means `observed_owner` describes the last authentication on the current
   session. Neither status performs a liveness check.
2. Call `mynofy_poll_next` with `{"topic":"project/work","limit":20}`. If
   `structuredContent.type` is `poll_deferred`, schedule the next attempt
   using `retry_after_ms`. The default interval is 30 seconds. The workflow
   or human can set `interval_ms` from 1,000 to 3,600,000. Calls made during
   a cooldown return immediately without polling the broker.
3. Process the page or record the pending work, then call
   `mynofy_checkpoint_commit` with its topic and `checkpoint_receipt` as
   `receipt`. You may also supply a `state` object of up to 16 KiB, for
   example `{"ticket":"TASK-001","phase":"reviewed"}`. Omission retains the
   existing state, and `state: null` removes it. The whole file is limited to
   4 MiB. `checkpoint_too_large` reports the attempted and maximum size and
   leaves the previous checkpoint and receipt intact. Reduce the proposed
   state or free space before retrying. A committed page with `has_more: true` allows immediate
   catch-up. An empty page can also be committed.

The local file is private, contains plaintext application state, and is locked
to one process. Checkpoints are bound to the authenticated
owner and client. Each topic has its own cursor and workflow state. A newer
successful poll replaces the previous receipt for that topic. At the 1,024-topic
in-memory tracking limit, entries whose cooldown has ended are reclaimed,
including their outstanding receipts. Durable cursor and state maps each
hold up to 1,024 topics. A process restart
discards pending receipts, so uncommitted pages can be read again. An owner boot
change clears saved read positions and pending receipts while preserving
workflow state. Use `replay: true` on a managed poll to deliberately read from
the beginning, for example after topic recreation. Reconcile replays with the
application's own records before acting.

The MCP host controls workflow deadlines and cancellation. It can wait between
attempts without occupying the MCP connection. Cancellation during a checkpoint
write can leave a committed result. Read checkpoint status to reconcile it.
Commit checks the owner with a read-only request before writing locally.
A lost connection or newly observed owner boot prevents that receipt from
advancing the checkpoint. The probe consumes one authenticated request
and verifies the session at the time of the reply. Checkpoint writes do not
encompass broker publishes or external side effects.
`mynofy_poll` remains available for explicit cursor control.

<a id="exakte-64-bit-werte"></a>

## Exact 64-bit values

ACK/NACK tools and delivery IDs have been removed. Application acceptance and rejection are published events. Poll is read-only.

Typed event parameters `u64` and `i64` are also displayed as decimal strings in MCP results.
Publish accepts canonical decimal strings and converts them back losslessly before the protobuf
call. Large numerical 64-bit inputs are rejected; normal small numbers are allowed. Any legacy
JSON payloads are not recursively rewritten; for any byte-accurate data, the
Bytes-/Base64-Variante is still available.

Examples of tool arguments:

```json
{"topic":"project/work","payload":"TASK-001: Code ready for review"}
```


<a id="mcp-protokoll-und-betriebsgrenzen"></a>

## MCP -Protocol and operational limits

Revisions 2026-07-28 are supported with metadata per request and `server/discover` and
2025-11-25 with `initialize` and `notifications/initialized`. This is based on the official
specifications for
[versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning),
[stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio),
[Discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover) and the
[Older life cycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

For modern requests, `params._meta["io.modelcontextprotocol/protocolVersion"]` and
`params._meta["io.modelcontextprotocol/clientCapabilities"]` are required in each request.
Successful modern results carry `resultType: complete` and server information under `_meta`. A
modern request does not take log metadata from previous calls. Only the older mode uses the
explicit initialization step. MCP Client information is not a mynofy client identity;
authentication only uses the mynofy access data provided at startup.

Discovery, ping and tool list work without reachable brokers. The first professional tool
request establishes the owner connection if necessary. After a transport error, it is discarded;
a later explicit call can reconnect. The failed operation is **not repeated**. If a mutation
response is lost, the adapter reports `outcome_unknown: true`. Cancellation and process
termination also do not roll back an already accepted publish. The caller must check the state
before a new attempt.

For each process, at most one owner operation runs simultaneously; further technical calls
receive a `busy` tool result and are not included. Metadata requests remain operable.
`notifications/cancelled` terminates the affected local call, closes its connection and
suppresses the subsequent response. EOF or SIGINT/SIGTERM does not allow a separate owner
operation to continue in the MCP process.

Limits: a JSON - RPC line up to 64 KiB, limited input queue, response up to 16 MiB and write
deadline on stdout. Overlong lines are completely discarded so that the next request is still
framed correctly. The existing technical connection deadlines remain valid; they are not a
professional waiting period for the agent workflow.

For manual polling, the caller supplies `after` and saves the cursor with
the owner boot ID. For managed polling, MCP selects the stored cursor and
saves progress only when the caller commits the page. Both paths read active
messages without creating broker-side reader state. Workflow deadlines and
interpretation renewal belong to the caller. Topic input is limited to 256
UTF-8 bytes. JSON Schema counts Unicode characters for `maxLength`; the
runtime enforces the byte limit.

<a id="verifikation"></a>

## Verification

Unit tests check protocol modes, metadata, strict arguments, non-execution of tool
notifications, lossless 64-bit values, and limited framing. CLI/broker integration tests cover tool discovery, ACL rejection, manual and managed polling,
offline checkpoint recovery, parallel metadata, busy responses, cancellation,
EOF and an already accepted publish with intentionally lost answer without retry. MCP -Access
via the three-service owner- TLS -Kette getestet.

```sh
cargo test --locked -p mynofy-cli mcp
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

A specific Chat-/MCP-Host-application was not connected and no network performance measurement
made.The tests use their own JSON - RPC test client on the real stdio transport and existing
Broker-/SDK-Pfade.

## Payload typing and process shutdown

Integer-to-string conversion uses the original Protobuf event variant. Legacy JSON Data stays opaque even when it contains `kind: event` or event-shaped parameters. MCP shutdown completes active-operation cancellation before bounded CLI runtime shutdown, so SIGTERM can terminate the process while stdin stays open. See the [key rotation guide](key-rotation.md).
