# Broker protocol v1

The owner stores active messages by topic. Clients supply their last known
position when polling. Direct and tunneled clients use the same binary
broker protocol.

See the [schema](../protocol/v1/frame.schema.json), [examples](../protocol/v1/examples.json),
[polling fixtures](../protocol/v1/poll-examples.json), and
[producer inspection fixtures](../protocol/v1/status-examples.json).
[Getting started](getting-started.md) · [Service networking](service-network.md)

<a id="umfang-und-verbindung"></a>

## Scope and connection

A topic owner is also the ACL holder. It manages active messages in the RAM, an ordered topic
index and its highest acceptance level. The prototype uses binary protobuf frames via
authenticated WSS or explicitly configured local Unix sockets, exact topic names, immediately
answered polling, time-based expiration and producer queries. Optional content persistence is
implemented. Server-side long polling and scheduling are not planned; spontaneous push delivery,
automatic retries, wild cards and exclusive job assignment are not included. A later REST
connection can map the same operations.

Each binary WebSocket message carries one Protobuf frame. JSON -text commands are rejected with code
1003. The following examples show the readable CLI-/SDK-representation: Each JSON object has
`version: 1` and `type`. Additional fields and unknown types are rejected. Optional fields are
omitted instead of `null`. IDs are opaque, not empty strings; command IDs are unique per
connection. Message IDs identify active records within one owner. A topic is a non-empty,
upper/lower case sensitive string; `/` has no built-in hierarchy or wildcard semantics.

Before application frames, the [Client-Keypair-Handshake](client-auth.md) must be completed on
the same connection. The owner binds pubishes to the stable client ID determined from them. The
ACL action `subscribe` in this model means the permission to poll a topic. There is no
`subscribe` / `unsubscribe` command and no permanently registered topic login. All application
examples begin after successful authentication.

The [Konfigurationsdatei](configuration.md) delivers keys, ACL, topic policies, routes and rate
limits. runtime state remains in the RAM and is not automatically written back.
[permissions and routing guide](topic-routing.md) separates the owner's decision from
cached signs.

## Frames

All fields are mandatory unless designated as optional. `version` and `type` are added.

| Type | Direction | Fields | Meaning |
| --- | --- | --- | --- |
| `publish` | Producer → Owner | `id`, `topic`, `payload`; optional `expires` | Create a new message. |
| `result` | Owner → Client | `request_id`; with Publish additionally `message_id`, `expires` | The owner accepted or completed the command. |
| `poll` | Client → Owner | `id`, `topic`; optional `after`, `limit` | Request active messages for a position. |
| `messages` | Owner → Client | `request_id`, `items`, `next_cursor`, `has_more` | Limited poll response, even empty. |
| `list_messages` | Producer → Owner | `id`; optional `topic`, `limit`, `cursor` | List your own active messages. |
| `messages_page` | Owner → Producer | `request_id`, `items`; optional `next_cursor` | Page of your own news overviews. |
| `get_message` | Producer → Owner | `id`, `message_id` | Request your own active message and payload. |
| `message_status` | Owner → Producer | `request_id`, `message`, `created_at`; optional `deliveries` | Own content; technical delivery counter only with approval. |
| `get_permissions` | Client → Owner | `id`, `topic` | Ask for your own current topic rights. |
| `permissions` | Owner → Client | `request_id`, `topic`, `owner_id`, `acl_revision`, `actions` | Current information, no capability token. |
| `error` | Owner → Client | `code`, `detail`; optional `request_id` | Error without guaranteed processing. |

Query responses are immediately the confirmation of the command, without additional `result`.
Spontaneous `message` or `receipt` frames are not provided in the pull prototype; producers
request statuses themselves.

<a id="topic-polling-mit-cursor"></a>

## Topic polling with cursor

```json
{"version":1,"type":"poll","id":"poll-2","topic":"events","after":"00000002","limit":20}
```

```json
{"version":1,"type":"messages","request_id":"poll-2","items":[],"next_cursor":"00000002","has_more":false}
```

Poll cursors consist of exactly eight ASCII characters of the alphabet `0–9A–Za–z`, about
`00000002`. Clients store the returned value per owner and topic and send it back unchanged.

- **Without `after`:** Read from the start of the still active topic stock. Also previously
  confirmed messages may reappear. This is a conscious new query, not a server-side push on the
  reconnect.
- **With `after`:** Read active messages strictly according to the acceptance position contained
  in the cursor.The client software stores the last successfully processed progress and sends it
  back to the next poll.
- **Answer:** `items` are ordered ascending by acceptance position. Each entry contains
  `message`; there is no delivery ID. The technical message contains `id`, `topic`, `payload`
  and the effective sequence `expires.time`.
- **page size:** `limit` is between 1 and 100, default is 20. In addition, the configured
  response byte limit applies. Fewer entries than `limit` are allowed. A single accepted content
  must fit into a response, including all output metadata.
- **Progress:** The owner forms a consistent acceptance status H. `next_cursor` is behind the
  positions issued or skipped. After checking to H, `has_more: false` and the cursor is allowed
  to advance to H. At [budgetierten expiration cleanup](budgeted-expiration.md), a maximum of 256
  positions are checked: A blank page with `has_more: true` and advanced cursor is possible. The
  flag then does not guarantee any further active message, but still unchecked positions. The
  next query uses the received cursor.
- **Empty Answer:** There are no currently active newer messages. This does not mean that
  nothing has ever been published since the old cursor: messages may have already expired. There
  is no guarantee of completeness for a longer offline client.

<a id="quick-check-und-geordneter-index"></a>

### Quick-check and ordered index

The topic owner assigns internally consecutive acceptance positions and keeps the last status
for each topic. After Auth-/ACL-verification a valid cursor on the current status allows a comparison
without passing through the message stock: immediately answer empty. For positions behind the
current status or cursor recognized as invalid, the service starts again at the active
beginning; response and page limits as well as Auth/ACL apply unchanged.

For an older cursor, the ordered index is set directly by the position instead of checking each
active message. The goal is a logarithmic search plus work proportional to the page read; this
is an architectural intention, not yet a measured performance statement.

The poll cursor is a simple index without a signature, MAC or binding to a client. The topic
selection comes from `topic`; authentication and current ACL are checked separately for each
query. A client may deliberately choose a different valid position or share a position with
another authorized client. The server does not store any progress per subscriber. A message ID
is not a poll cursor: positions are valid per topic and remain usable even when the anchor
message expires.

Internally, the owner counts for each topic with `u64`; the last eight alphabetical digits
appear on the wire. `00000000` first indicates the status before the first message. `zzzzzzzz`
is followed by `00000000`. During the evaluation, the owner selects the latest suitable position
that is not behind his current status. An unrepresentable backlog before position 0 and invalid
length/character set the poll start to the active start. The current cursor does not need a
stock search, older positions are searched in the ordered index.

**Limit of the compact index:** After 62^8 assumptions, an old index is no longer unique.
Clients are not allowed to keep their progress over a full cycle. Restart and reinvestment reset
positions. An old position initially triggers a return to the active start in the future, but
can appear valid again after catching up with the new stock. The cursor does not reliably detect
such restarts; after a known restart, the application must deliberately start again without
`after`. No generation recognition or MAC guarantee for poll cursors claim. Previous long poll
cursors also trigger a return to the active start.

Producer-Listen-Cursor ( `list_messages.cursor` / `messages_page.next_cursor` ) remain separate
from it: they carry snapshot water level, filter and drain and are still Base64URL( JSON ) plus
HMAC - SHA256, tied to owner generation and client, but still has a limit of 4096 bytes. UTF
-8-Bytes begrenzt.

## Stateless polling and local progress

Topic names must contain 1–256 UTF-8 bytes. JSON Schema string lengths count
Unicode characters, so the runtime enforces the byte limit separately.

Poll reads active messages without creating reader state, confirmation pairs or delivery IDs. ACK/NACK commands are removed. Acceptance, rejection and completion are application events published to the agreed topics. See [upgrade notes](message-client-confirmation.md).

The client is not allowed to store a returned cursor until it has processed the related page
according to its application or reliably taken over locally; if it stores before and crashes, it
can skip messages; if it stores after and crashes before, the same work can reappear; if there
are several parallel polls of the same topic, the client must serialize its progress management
or merge the processing correctly; this prototype does not define atomic coupling of processing
and local checkpoint.

The [Polling Examples](../protocol/v1/poll-examples.json) shows pages, empty replies, an anchor
that has already expired and then a new publish. Immediate polls should be combined with
client-side Pause/Backoff for empty replies. Concrete intervals and limits have yet to be
specified. The service does not keep poll requests open until a new message; waiting intervals
and repetitions are with the client.

<a id="ablauf-und-aufbewahrung"></a>

## Processing and storage

The producer can set `expires: {"time":"2026-12-01T12:00:00Z"}`. Without specifying, the topic
owner determines the time from his topic policy with finite standard TTL. Invalid, already
reached or too long by policy specifications are rejected, not tacitly shortened. Publish
result, poll result and own queries always contain the effective time. Standard TTL and maximum
runtime are set per topic in the CFG. Time resolution is milliseconds.

Any accepted message remains active until expiration in the RAM. Missing readers do not delete
them. Policy changes do not retroactively change accepted times. Decisive is the owner clock; an
absolute RFC -3339- UTC -time with `Z` is prescribed.

When it expires, the content immediately becomes logically invalid. The physical cleanup is
budgeted in several steps. Quota slots are released only when it is actually cleaned up. Content
that has already been transferred cannot be recalled. Topic acceptance status is retained even
after all messages have expired. Topic reset and a restart without content checkpoint reset the
topic acceptance status. A full content restore receives stored topic headstands and remotely
expired messages before output. Without saved status, the RAM stock is discarded. Each restart
generates a new owner generation for producer list cursors. There is no history of expired
content.

<a id="eigene-aktive-nachrichten-und-status"></a>

## Own active messages and status

Active means accepted, not yet expired and has not since been restarted. Producers – even
self-publishing services – can continue to read their original active content when validly
registered, regardless of remaining topic rights. Foreign replies remain separate messages and
require `subscribe` to poll; re-publish requires current publish rights.

`get_message` returns the original content and `created_at`. Foreign, unknown or expired IDs return `not_found`. `list_messages` returns the producer's own active messages, optionally filtered by exact topic, with `id`, `topic`, `created_at` and `expires`. Neither response contains reader or confirmation counters. These queries remain available to authenticated producers after losing topic rights.

Producer lists can also deliver blank pages with `next_cursor` because of the scan budget; only
the empty entry list is not an end criterion. Producer lists use their own paging: `limit`
1-100, standard 20, plus response byte limit. The list cursor binds identity, topic filter, page
size and an acceptance water level. Subsequent pages skip now expired entries and contain no
later publishes. Status data can change between pages. Topic filters and page size remain
unchanged. The cursor of a producer list is not a topic poll cursor and vice versa.
`next_cursor` is omitted at the end of the list. Producer list cursor applies in the prototype
60 seconds and is additionally bound to the ACL revision.

<a id="bewusst-erneut-veröffentlichen"></a>

## Deliberately re-publish

A producer can use their own queries to decide to re-publish using a new `publish`, creating a
new message ID and a new sequence; the old message remains active. There is no deletion
operation, no automatic retry, and no `resend` under the same ID. The professional run
identifier can be retained for the same job for the application to detect repetitions. A new
identifier is required for an independent workflow.

Query, Retry and a possibly late result are not atomic. RAM -Retention, cursor and status do not
guarantee at-least-once processing or exact-once effects. If the publish result is lost, the
output may remain unclear. [Status examples](../protocol/v1/status-examples.json) shows
deliberate retry and content query for rights revocation. All times and cursors of the examples
are illustrative values, not an immediately executable configuration.

<a id="ressourcen-und-vernetzung"></a>

## Resources and networking

The owner stores each accepted publish once. Polling reads active history.
For N messages and S full readers, their polls can transfer the content
N×S times. The owner applies limits to active records and bytes, response
pages, concurrent commands and request rate. If necessary, a request
fails with `resource_exhausted`. A revoked session cannot send a protected
reply that has not already been released.

[Peer Identity and Handshake](service-links.md) remain separate from the client protocol. Owner-/Next-Hop-resolution and permissions information are described in [topic-routing.md](topic-routing.md). The original client authenticates itself to the owner via a mediated, owner-terminated session. The implemented tunnel carries the original client protocol to the owner without reader-confirmation state. An intermediate entity may not make itself a producer or identify a hop as a confirmed final receipt by forwarding. Global producer queries about multiple owners are not part of the prototype.

## error codes

| Code | Meaning |
| --- | --- |
| `invalid_frame` | Invalid JSON, field structure or expiration outside the policy. |
| `unsupported_version` | The version is not supported. |
| `unknown_type` | Type is not supported. |
| `invalid_direction` | Type was sent in the wrong direction. |
| `not_found` | Your own active message cannot be found. |
| `invalid_cursor` | Producer list cursor invalid, incorrect scope, no longer valid generation or inadmissible position. |
| `unauthorized` | Client identity is missing or invalid. |
| `forbidden` | Topic operation is not allowed. |
| `resource_exhausted` | Resource limit prevents the operation. |
| `security_storage_busy` | Security writing queue full; no change in security status. |
| `security_storage_unavailable` | Security writer failed; details about the RAM-/stored state provides the admin diagnosis. |
| `rate_limited` | client or source budget is reached; `retry_after_ms` indicates the waiting time. |
| `owner_unavailable` | Responsible owner unavailable; no local replacement release. |
| `route_conflict` | Conflicting topical responsibilities or impermissible path. |
| `internal_error` | Internal error without sensitive details. |

<a id="implementiertes-protobuf-format"></a>

## Implemented Protobuf format

[Scheme and migration](protobuf.md) describe binary protobuf frames with oneof message bodies, Bytes-/Event-Payloads and the readable JSON representation for CLI/SDK. The JSON examples shown here remain the readable representation of the CLI. Identity, ACL, cursor, execution and technical status rules apply to the protobuf client transport and the internal readable representation.


<a id="implementierter-clientseitiger-schlüsselwechsel"></a>

## Implemented client-side key change

On an already authenticated WSS or local Unix socket connection, the following applies:

| Request | Response | Fields in addition to Version/Type and Command-/Request-ID |
| --- | --- | --- |
| `rotation_start` | `rotation_offer` | Antwort: `rotation_id`, `expires_at`, `new_valid_until`. |
| `rotation_confirm` | `rotation_ready` | Request: `rotation_id`, `shared_secret`; Antwort: `rotation_id`, `nonce`, `challenge`, `aad`, `expires_at`, `new_valid_until`. |
| `rotation_commit` | `rotation_complete` | Request: `rotation_id`, `encrypted_public_key`, `signature`; Antwort: `rotation_id`, `client_id`, `token_id`, `valid_until`. |

The client here is the initiator and generates the 32-byte shared secret after the offer. It is
transmitted canonically base64-encoded in the confirm and is not returned by the service. The
service provides a random 12-byte AES - GCM -nonce and its own 32-byte proof challenge. The new
public key is encrypted with AES -256- GCM; the authentication tag is attached to the
ciphertext, then base64-encoded.

AAD is the compact UTF -8- JSON -Tupel `["mynofy-rotation-v1", owner_id, stable_client_id,
rotation_id, old_token_id, new_valid_until_unix_ms]` Clients construct this tuple themselves and
compare the bytes returned by the service in base64-encoded form for orientation.
`ASCII("mynofy-rotation-proof-v1") || 0x00 || aad_raw || new_public_key_raw_32 ||
challenge_raw_32`.

Failed commit consumes the appropriate process together with Secret/Nonce. A new attempt must
start with a new change ID. Deadline and revocation prevent a still open commit; a completed
commit is not rolled back. The new validity follows the owner-CFG (`key_validity_seconds` ).
Server-initiated offers are not provided; the rotation is initiated by the client.
`rotation_in_progress` and `rotation_invalid` supplement the error codes.
`security_storage_busy` / `security_storage_unavailable` can also prevent the commit; an already
consumed process must be restarted. A successful commit applies immediately in RAM. With the default
`security_storage.sync: true`, the success response confirms journal
synchronization. [Security journal](security-journal.md) ·
[Error examples](../protocol/v1/security-storage-examples.json)

The local prototype assigns message IDs as eight-digit ASCII counters from the alphabet
`0–9A–Za–z`, owner-local from reserved areas. They are unique within the active owner inventory,
can be reused after expiration, circulation and restart, and are not proofs of authorization.
Applications must not derive poll progress from their order. Poll and producer-list cursors have separate formats.

## event catalog abrufen

`get_catalog` is an authenticated metadata query with `id`, `name` and `catalog_version` (numerical). The owner responds with `catalog`, `request_id` and the full definition under `catalog`. Currently available: `mynofy`, version `1` (draft). No topic right is required; client rate limit and response budget are unchanged. Unknown Name/Version: `not_found`. Server frames of type `catalog` are not client commands. This is neither an automatic handshake comparison nor a registration of own catalogs.

The definition including parameter types is described in the
[event catalog](event-catalog.md#vom-client-abrufen). The 4096 bytes there limit free
payloads; they do not limit this metadata response. Previous JSON payloads are stored compatible
as JSON bytes and are also subject to the 4096 byte limit.
