<a id="primärer-mcp-zugang"></a>

# Primary MCP access

Use the mynofy MCP tools provided. Host-dependent prefixes can be placed before the tool names
mentioned here. MCP -Transport and protocol handshake is handled by the host; do not send JSON -
RPC via message topics yourself.

If no MCP connection is provided, use the [CLI-Ersatzweg](cli.md). An existing MCP access with
an error is not a reason for an automatic interface change or a second publish.

<a id="einstieg-und-verfügbare-operationen"></a>

## Entry and Available Operations

`mynofy_connection_info` delivers under `structuredContent` owner, stable
Client ID, restart counter and boot ID (`boot_id`) if provided by the owner.
`response.owner_context` distinguishes a new authentication from cached data. Compare this
assignment to the intended work context. The data of an already loaded session is not its own
liveness proof. Access data and owner TLS check are configured in the MCP process; do not pass
keys or a freely claimed client ID as tool arguments.

| Purpose | Tool and arguments |
| --- | --- |
| Retrieve a well-known catalog | `mynofy_get_catalog`, optional `name`, `catalog_version` |
| Checking own topic rights | `mynofy_get_permissions` with `topic` |
| Publish once | `mynofy_publish` with `topic`, `payload`, optional `expires.time` |
| Reading a page | `mynofy_poll` with `topic`, optional `after`, `limit` |
| List your own active messages | `mynofy_list_messages`, optional `topic`, `limit`, `cursor` |
| Read your own original message | `mynofy_get_message` with `message_id` |

Broker responses are under `structuredContent.response`; owner, client ID and restart counter
and, if applicable, boot ID. If the host only delivers text content, the same envelope is
included there as JSON. A `isError` result is not a successful professional action. evaluate
broker errors and rate limits, do not bypass other tools or invented rights. There are no admin
tools.

<a id="polling-und-exakte-referenzen"></a>

## Polling and Exact References

If the host supplied `--checkpoint`, load `mynofy_checkpoint_status` per topic
and use `mynofy_poll_next`. If the tool returns `poll_deferred`, wait the
returned `retry_after_ms` before polling that topic again. Use the agreed
interval (30 seconds by default). Pass the page's `checkpoint_receipt` as
`receipt` to `mynofy_checkpoint_commit` after handling the page or recording
pending work. Include local ticket state when it should advance with the
cursor. After cancellation during a checkpoint write, consult checkpoint
status before repeating work. Status reads the local file state without a
broker call. Use `committed_cursor` and `committed_owner` for recovery and
treat cached or unchecked owner data as historical. Commit verifies the
owner session. If the file budget is exceeded, reduce the proposed state
before retrying the same still-valid receipt.

For manual polling, retain `next_cursor` with the boot ID per owner and
topic and pass it as `after` on the next poll for the same boot. Use paging
and keep workflow state locally. Reconcile replayed messages before performing
external actions. `busy` means the adapter did not start another owner call.

The broker has no ACK/NACK tools or delivery IDs. Report business acceptance and rejection by
publishing an agreed event. Typed event parameters `u64` / `i64` can also be delivered as exact
decimal strings and used by Publish.

<a id="fehler-cancellation-und-wiederverbindung"></a>

## Error, Cancellation and Reconnection

A response with `outcome_unknown: true` means the owner may have
accepted the mutation before the response was lost. Check active own
messages and local progress before deciding whether to issue a new
request. An absent or expired message does not establish the outcome.

A later explicit call can reconnect. Verify the owner and client identity.
Compare the current `boot_id` with the ID stored alongside the cursor.
If it changed, or if the field appeared or disappeared, poll without
`after`. For older owners without a boot ID, use changes to the restart
counter as the replay signal. Validate the owner context in poll
responses too. If it changed during the call, repeat the read without
the previous cursor. MCP cancellation cannot undo applied effects.

The rules on roles, human orders, tickets, interpretation and stop are in
[workflow rules](workflow.md). MCP does not grant any additional rights and does not implement its
own workflow or permanent ticket status.
