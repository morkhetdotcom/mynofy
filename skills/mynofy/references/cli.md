<a id="cli-ersatzweg-für-den-entwurf"></a>

# CLI -Replacement path for the design

Use this path only if no MCP link is provided, not to automatically repeat failed MCP calls. Use
a provided `mynofy` binary. These examples show existing interfaces; actual owners, topic names,
and paths come from the job. Private keys are specified as file path, not published in the
topic.

Directly via a protected local socket:

```sh
mynofy client --socket /provided/mynofy.sock --key /provided/agent.pk8 --owner owner
```

Directly from TLS/WebSocket:

```sh
mynofy client --url wss://provided-host:8092 --ca /provided/ca.crt --key /provided/agent.pk8 --owner owner
```

From a provided entry point to the remote owner:

```sh
mynofy client --url wss://provided-proxy:8090 --ca /provided/proxy-ca.crt --key /provided/agent.pk8 --owner owner --via-topic project/work --owner-ca /provided/owner-ca.crt
```

For different owner certificate name `--owner-name` After the key rotation, it is possible to
`--client-id` Indicate the already existing stable identity. CA-/Owner-mapping erfinden.

The CLI reads JSONL from stdin and provides the respective answer as JSONL on stdout. It does
not generate a poll loop and does not generate regular announcements by itself. An executive
agent or a suitable client driver has to control intervals and deadlines; such a skill helper
has not yet been built.

Examples of individual requests on an authenticated connection:

```json
{"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}
{"version":1,"type":"get_permissions","id":"permissions","topic":"project/work"}
{"version":1,"type":"poll","id":"poll","topic":"project/work","limit":20}
```

For the next poll of the same topic, use the delivered `next_cursor` as `after`. `has_more` and
Seiten-/Byte-Limits. Keep the cursor separate per owner and topic. Human, operator and agent
topics are read independently, without common atomic order.

The broker has no ACK/NACK commands or delivery IDs. Publish acceptance, rejection and completion as separate messages using the negotiated application protocol. Store read progress locally; losing a cursor can replay active messages.

Publish uses existing payload variants: known typed events or free content up to 4096 actual
bytes. CLI supports JSON compatibility representation; its own Ticket-/messages-wire format is
not redefined here. Large artifacts do not belong in the message.

`--progress FILE` stores under Unix Poll progress and owner reboot knowledge.
It does not store complete ticket processing, does not only prevent duplicate trade promotions
and is not a global stop register. After a known owner restart, active messages can still be
read again. The existing binary adapter confirms triggers before execution and is not a
substitute for permanent agent work.
