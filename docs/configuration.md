<a id="konfiguration-und-ram-betrieb-des-prototyps"></a>

# Configuration and RAM operation of the prototype

Preferred format is YAML (`.yaml` / `.yml`); existing JSON remains supported. Both formats have
the same types and checks. [YAML - Profile, export and conversion](yaml-config.md).

Current model: [stateless polling and upgrade notes](message-client-confirmation.md). ACK/NACK, delivery IDs, confirmation counters and pair limits have been removed; update services and clients together.

Messages, sessions, incomplete key changes and rate limiters live in RAM.
Clients, keys, revocations and local ACLs use the separate
[security journal](security-journal.md). With the default `sync: true`,
a successful service security-mutation reply confirms a durable write.
Optional [content saves](content-storage.md) persist messages manually or
periodically. Restore and optional decryption happen during startup.

The [scenario files](../config/scenarios/local.yaml) are templates. Convert
one with `mynofy config-convert --config TEMPLATE --out service.yaml` to add
the compatible security directory, then adapt the generated config to your
deployment. `mynofy check-config --config service.yaml` validates the
configuration. Before serving, initialize the configured security store
with `mynofy security-init --config service.yaml` and provision the specified
listener paths and credentials. The [local setup](getting-started.md)
performs these steps for a Unix socket. Network modes require matching
certificates and trust assignments. See the [networking guide](service-network.md).

| File | Scenario |
| --- | --- |
| [unix.yaml](../config/scenarios/unix.yaml) | Local Unix socket without TLS files; Linux/macOS. |
| [local.yaml](../config/scenarios/local.yaml) | Individual owner with local test topic. |
| [central.yaml](../config/scenarios/central.yaml) | Owner for a shared topic with own ACL. |
| [edge.yaml](../config/scenarios/edge.yaml) | Own local topic and remote topic via the central service. |

With the exception of the fixed maximum 4096 payload bytes, the numbers are configurable test
values, no measured capacity limits or immutable product defaults. The file loads service ID,
Listener/TLS, known client-public keys, ACLs of the self-responsible topics, topic policies, an
optional upstream, trust assignments and resource limits.

The example client uses only the public key from the Auth test data. Its private key has been
discarded. For an executable test, a new client key pair must be created and its public key
entered; client ID and token ID must be derived accordingly. TLS files are local file references
and must be provided before a later start. No client private keys or shared secrets are stored
in scenario files.

<a id="start--und-reset-regeln"></a>

## Start and Reset Rules

- Startup reads operational parameters and topic definitions from the config.
  The runtime restores clients, keys, revocations and local ACLs from the
  initialized security snapshot and journal. The config supplies the baseline
  at initialization. A damaged store prevents startup.
- Admin status reports submitted and durable journal sequences. The config
  file is unchanged by runtime security mutations.
- Without content storage, messages start empty. A content checkpoint restores
  stored messages with their original expiration times. A new owner boot
  invalidates producer-list cursors. Restart topic polling without `after`
  when the owner boot changes.
- `security_storage.sync: true` waits for durable journal synchronization before successful service security-mutation replies; default `true` confirms durable writes; explicit `false` allows asynchronous acknowledgement. Message persistence is independent. See [security journal](security-journal.md).
- If content persistence is configured, the service requires a `content_storage.store_id` and a
  volume set up with `mynofy content-init --config FILE`. A missing or incorrectly assigned
  volume prevents it from starting. [Content-Storage](content-storage.md).
- The CLI service requires `security_storage.directory` as an explicit absolute or CFG relative
  path and an already initialized store. The initial setup takes place with `mynofy
  security-init --config FILE`. Queue and compaction parameters and file rights are in
  [Journal-Leitfaden](security-journal.md).

## rate limits

Each authenticated client receives a common rate limit across all of its connections. Key change
and Reconnect do not reset this budget because it is tied to the stable client ID. In addition,
there is a dedicated budget for started key change; only one change per client may be active at
a time. Repeated attempts after deadline also consume budget.

Before successful authentication, a claimed client ID is not yet a trusted rate limit key, so
this phase requires additional limitation per connection source and for the entire listener, and
unknown token IDs may not generate any number of separate budgets or consume a real client
budget without proof of identity.

Foreseen is `rate_limited` with `retry_after_ms`; rejected commands do not generate messages or
delivery attempts. Implemented is one token bucket per stable client ID for commands (one token
per frame, even in the case of faulty requests); prior to Auth, Quellen-/Listener-Grenzen. key
changes additionally have a sliding trial window. Client budget and expiration use the core time
in milliseconds, source limits monotonous transport time. Crossing boundaries already before
TLS/Auth leads to connection disconnection, because no JSON response is possible there. Rates
limit work, but do not replace limits for bytes, parallel connections and RAM. They are not a
guarantee against arbitrarily large distributed traffic.

<a id="weitere-testszenarien"></a>

## Further test scenarios

From the baselines, tests for expiration times, empty polls, loss of rights with still readable
property, change timeout, rate limit over several sessions, owner failure and outdated routing
tables are derived. The files define inputs; corresponding service integration tests are only
possible with the implementation.

<a id="lokaler-unix-listener"></a>

## Local Unix Listener

`service.unix_socket` replaces `service.listen` and `service.tls`; both variants are invalid at the same time. Relative socket paths are resolved relative to CFG. [setup-unix.sh](../scripts/setup-unix.sh) creates a separate local test bench below `.local/unix` without changing an existing WSS configuration. Keypair-Auth and Topic-ACL still apply.

## Payload-Migration

`limits.max_payload_bytes` accepts 1–4096. Reduce any older 65,536-byte limit before restarting. The limit covers the raw payload, and the response has a separate byte budget. Setup scripts preserve existing `.local` configurations.

For the [Separate admin socket](admin-api.md), `service.admin_socket` is optional. It must be
separate from the data socket and is resolved relative to the CFG file. Client entries support
`disabled: true`; with no indication, false applies. After full key revocation, an identity
entry with `keys: []` is allowed, but without data login. An explicit admin export contains the
current identity/ACL status; no automatic write-back occurs.

<a id="speicherstrategie-pro-topic"></a>

## Storage strategy per topic

`content_storage.directory` provides the optional content baking. Local topics remain without indication `storage: {mode: ram}`. Add `storage: {mode: manual, encrypted: true}` or `storage: {mode: interval, seconds: 60, encrypted: false}` for explicitly stored topics. Encrypted topics use the common start secret from `content_storage.encryption: password_stdin` or `key_stdin`. The former global `save_interval_seconds` is omitted. Group saves and migration of old checkpoints: [Content-persistence](content-storage.md).


See the [service networking guide](service-network.md) for CA-based mTLS operation.
`scripts/setup-links.sh` generates a complete local three-service example with matching
certificates and YAML -Dateien. `limits.max_service_channels` Limits channel resources per
instance (Default 64, 1–4096, effective maximum) max_connections ); passing clients require an
incoming and an outgoing channel at an intermediate station. TLS -Information data for the
outgoing service connection.


## Publisher and topic capacity

`limits.max_publisher_active_bytes`, `limits.max_publisher_active_messages`,
`limits.max_topic_active_bytes` and `limits.max_topic_active_messages` take
positive integers. Omitted limits default to half of the corresponding owner limit, rounded up. The default byte limit allows one valid message even if the owner limit was set unusually low. Publishers are counted across topics and topics across publishers. The owner limit still applies to every publish. A rejected publish leaves existing messages unchanged. Restored content counts against the current limits, and existing excess drains through normal expiry.


TCP admission retains inactive source rate history for 60 seconds and expires it in bounded batches. If the source table is full, new sources share a small overflow rate and connection budget. IPv6 sources also share a /64 connection limit, `limits.max_connections_per_ipv6_prefix` (default 64). Setting the field to null disables the prefix quota. The prefix also shares a rate budget equal to four times the per-source rate and burst.


### Optional transport response budget

`limits.max_egress_bytes` reserves one `max_response_bytes` allowance before
processing a client frame. Unix, direct WSS and tunnels terminating at the owner share the allowance. Durable rotation commits use four separate small response slots while waiting for the Security journal. Ordinary requests and responses keep using the normal allowance. This mode releases the encoding buffer after each response. Requests wait up to 100 ms for a slot and execute only after acquiring one. A separate reserve can send four simultaneous small rejection frames. Authenticated requests with a valid request ID receive a correlated `resource_exhausted` response when the wait expires, subject to normal rate limits. Admin status exposes `egress_rejections`. If the reserve is occupied or delivery fails, the connection can close. The allowance covers logical encoded-response capacity. Allocator slack, kernel and TLS buffers, peer relay buffers, content capture and admin export have their own resource usage. Omitting the field preserves the existing per-session limits.

Producer-list continuation expires after 60 seconds without advancing. Every successful page with a continuation issues a new deadline. Topic, identity, scope, authorization revision and the original producer watermark stay bound to the continuation.


Authenticated WebSocket commands consume one identity-rate token before full Protobuf decoding. Admission can still read bounded frame metadata. The owner checks current credentials again immediately before processing the command.
