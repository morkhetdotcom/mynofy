<a id="lokale-admin-api"></a>

# Local admin API

The administration API is implemented and uses its own Unix socket. It manages identities, keys
and ACLs in the current owner. It is not part of the message log, nor is it accessible via
topics, REST or the WSS port. It works alongside a WSS data lister or a local data unix socket.

## Basic status and explicit storage diagnostics

```sh
# Liveness and readiness without inventory scans.
mynofy admin health --socket .local/unix/run/mynofy-admin.sock

# Detailed service and storage status.
mynofy admin status --socket .local/unix/run/mynofy-admin.sock

# On-demand record, payload, arena and index statistics.
mynofy admin storage stats --socket .local/unix/run/mynofy-admin.sock

# Inspect save progress for one persistent topic.
mynofy admin content status TOPIC --socket .local/unix/run/mynofy-admin.sock
```

`health` returns `ready`, `service_id` and `restart_counter`. It checks the local admin endpoint and broker readiness. Use `status` for storage or network diagnostics.

`status` reports service identity, client/key/session/topic counts,
`resident_messages`, charged message bytes, limits, security/content persistence
health and service links. `resident_messages` is the constant-time resident record count, including expired records awaiting normal bounded cleanup. It is not an exact active-message count. Session validation and configured metadata processing remain; this is not a constant-time claim for arbitrary metadata sizes.

`storage_stats` scans resident messages to report active/expired counts, payload
bytes, record layout, allocated arena/pages and index sizes. It holds the owner lock for a consistent view, so use it on demand rather than as a frequent health check. Both commands are read-only and do not wait for security durability.

Admin JSON request (on the separate protected Unix socket):

```json
{"version":1,"command":{"op":"storage_stats"}}
```

The former `status.result.storage` object moves to `storage_stats.result`; the CLI prints that result directly. Update scripts that consumed detailed storage fields to use the explicit command. Routine save/journal monitoring keeps using `status` and its `content_storage` / `security_storage` fields.

## Aktivieren

In the existing start configuration under `service` add:

```json
"admin_socket": "run/mynofy-admin.sock"
```

The remaining service fields for the data lister remain necessary. The admin path must be
separated from the data socket. Relative file paths are resolved relative to the configuration
directory when loaded; export snapshots contain resolved absolute paths. Without `admin_socket`,
no admin lister starts. The setup scripts `setup-local.sh` and `setup-unix.sh` set the path for
newly created demos; existing local configurations are not changed.

```sh
cargo run --locked -- serve --config .local/unix/config.json
cargo run --locked -- admin status --socket .local/unix/run/mynofy-admin.sock
cargo run --locked -- admin help
```

The socket has mode `0600`. Newly created directories have mode `0700`; an existing parent
directory must not be writeable for group or others. The service also checks the UID of the
connected process against the socket owner UID. Existing files, symlinks and old socket entries
are not overwritten at startup. When quitting, only self-created socket files identified by
device/inode are removed; replacement files are retained. A non-bindable configured admin socket
makes the service start fail.

The management permission thus follows the operating system identity: processes under the same
UID as the service are allowed to administer. There are no other admin roles or admin tokens at
first. A normal client keypair or a topic ACL does not grant admin access. The CLI client checks
socket file type, mode and the UID of the connected server. The implementation uses Unix APIs
under `cfg(unix)`; the tests were run on Linux, no native macOS test was performed.

## Kommandos

All calls need `--socket PATH`; the following examples use a shortened placeholder `PATH`.

| CLI | Funktion |
| --- | --- |
| `mynofy admin clients list --socket PATH` | List client identities with name, lock, revision, key number and number of valid sessions. |
| `mynofy admin clients get CLIENT --socket PATH` | View public keys, token IDs, expiry and validity and session information. |
| `mynofy admin clients add --public-key FILE --socket PATH` | Register new stable identity from the first public key, initially without topic rights. |
| `mynofy admin clients disable CLIENT --socket PATH` | Lock the client and invalidate its sessions. |
| `mynofy admin clients enable CLIENT --socket PATH` | Re-allow client; new registration required. |
| `mynofy admin keys revoke TOKEN --socket PATH` | Revoke a registered key and invalidate its Sessions/Challenges. |
| `mynofy admin keys add CLIENT --public-key FILE --socket PATH` | Add a key explicitly assigned by the operator, for example for resetting after key loss. |
| `mynofy admin acl get CLIENT TOPIC --socket PATH` | Request rights of a client on an exact local topic. |
| `mynofy admin acl set CLIENT TOPIC --allow publish,subscribe --socket PATH` | Replace the full rights set for this pair. |
| `mynofy admin acl set CLIENT TOPIC --allow none --socket PATH` | Remove all rights of this pair. |
| `mynofy admin status --socket PATH` | Client-/Session-/Topiczahlen, resource occupancy, limits and adjustment backlog. |
| `mynofy admin export --socket PATH --out NEW_FILE` | Export current identity/ACL status as a new start configuration. |

`clients list` supports `--limit 1..100` (default 20) and `--after SLOT`. The response contains `items` and, if applicable, `next_after`. This administrative continuation index is not a client ID and is not a topic cursor; new requests for a service restart start without `--after`. `clients get` shows a maximum of 100 session IDs and total number and `sessions_truncated`.

Public key files contain either the JSON result of `keygen` or the raw base64-encoded 32-byte
public key. PEM/Zertifikate and private key files are not intended for this. `public_key` is
read at JSON; an existing `algorithm` specification must be `ed25519`. The key commands accept
`--valid-until RFC3339_UTC_Z`; without specification, the configured
`rotation.key_validity_seconds` applies from owner time. `clients add` additionally supports
`--name NAME` with a maximum of 256 UTF -8 bytes.

```sh
cargo run --locked -- keygen --out review-agent.pk8 > review-agent-public.json
cargo run --locked -- admin clients add \
  --socket .local/unix/run/mynofy-admin.sock \
  --public-key review-agent-public.json --name review-agent
```

The answer provides the new `client_id`. After that, give the required rights to this identity
and log in the normal client with its private key file. Registration does not replace the proof
of key ownership in the normal Auth handshake. No more than 10,000 identities and eight
registered keys per identity are allowed. Duplicate identities or already registered public keys
are rejected.

`keys add` adds one key and does not automatically revoke another. verlorene/kompromittierte Key recommends the explicit sequence: lock client, revoke old tokens, assign new public key, re-allow client. Stable client ID and ACL remain intact. `--client-id` must specify the existing stable identity when logging in with an administratively assigned new key. The usual authenticated rotation remains the preferred procedure if the previous key is still available and valid.

## Atomare effect

Mutations run under the same core lock as message operations. Inputs and capacities are checked
before the change. ACL Changes increase the revision and invalidate affected client sessions and
key changes. A lock also discards the previous sessions and pending changes; unlock does not
restore a session. Session parallelity limits are managed per client in the core, so that even
subsequently registered clients are immediately given the same limits.

A key revocation only removes the selected key and its sessions. Pending auth challenges are
also discarded so that they do not become valid again after a later explicit re-registration of
the same key. Other keys of the same identity remain usable as long as the client is not blocked
altogether. Even after revocation of the last key, the identity remains; `keys: []` is therefore
allowed in a start configuration. Without a registered valid key, no data login is possible.

Already stored messages and their producer identities are preserved by these management
actions. A validly newly logged-in client can continue to use its original active content and
previously received pair references according to the existing rules.

The disability directly affects new core operations. Network connections close when used further
or during authentic check; an already running shipment is additionally limited by the previous
network timeout. Answers already formed or transmitted before the change cannot be recalled.

Local topics and TTL policies can be changed via the [Topic-management](runtime-topics.md).
Resource limits, listeners and routing remain start configuration; there is no file hot reload.
Client identities are not deleted or slots renumbered as long as RAM content can still be
assigned to them.

## Expliziter Export

Security mutations act immediately in the RAM and are automatically included in the separate
[Security-Journal](security-journal.md). By default (`security_storage.sync: true` ), successful
security-mutation replies wait for the captured journal sequence outside the core lock. Explicit
`security_storage.sync: false` allows asynchronous confirmation. `security_storage_unsynced`
means RAM was changed but durable confirmation failed; inspect status before retrying. Read-only
requests remain immediately. A restart restores the synchronized security state; pending writes
can be lost in the event of a crash. The status contains `security_storage` with
eingereihter/dauerhafter sequence, queue assignment and memory error. A full queue rejects new
mutations before their effect with `security_storage_busy`. The original CFG file is never
automatically written back. An export contains the currently registered keys including completed
rotation, runtime registrations, blocking flags and current ACLs:

```sh
cargo run --locked -- admin export \
  --socket .local/unix/run/mynofy-admin.sock \
  --out config.snapshot.json
cargo run --locked -- check-config --config config.snapshot.json
```

`--out` creates a new file with private file privileges and does not overwrite an existing file. `.yaml` / `.yml` creates YAML, other endings JSON. Without `--out`, stdout JSON. [YAML Profile and conversion](yaml-config.md). The user can check it and later use it expressly with `serve --config config.snapshot.json`. File paths are absolute; when moving to another host they may need to be adjusted.

No private key file contents, rotation secrets, messages, sessions, cursor, rate
limiter or ongoing changes are exported. TLS file paths remain part of the configuration.
Revisions and owner generation are new RAM states after restart. The export is a consistent
metadata snapshot, not news persistence. It contains the security path and does not replace its
journal; it sets its own path for an independent copy. Metadata is copied under the core lock;
validation, serialization and Socket-/Datei-I/O afterwards. Very large metadata snapshots and
diagnostic scans are not constant-time message operations.

<a id="separates-protokoll-und-grenzen"></a>

## Separate Protocol and Borders

On the admin socket, there is a small JSONL protocol with exactly one request and one response
per connection, after which the service closes the connection, and it is not a WebSocket and is
not part of the normal protobuf frames.

```json
{"version":1,"command":{"op":"status"}}
```

```json
{"version":1,"ok":false,"error":{"code":"not_found","detail":"Client, key or local topic not found."}}
```

Success delivers `{"version":1,"ok":true,"result":{...}}`. The CLI outputs only `result`; for
errors, it ends with an error status. Unknown fields or operations and duplicate JSON keys are
rejected. The Rust input types, the [Schema](../protocol/admin/v1.schema.json) and the
[Examples](../protocol/admin/examples.json) describe the current version.

The listener admits up to six connections at once. Four slots are available
for ordinary commands, leaving capacity for `status` and `health`.
It accepts up to ten connections per second with a burst of 20. Requests
are limited to 16 KiB and responses to 32 MiB. The read deadline is two
seconds and the connection budget is nine seconds. Mutations must start
within seven seconds, which is also the deadline for any required security
confirmation. Oversized responses return `resource_exhausted`. The cost of
capturing a full configuration export depends on the number of entries.

There are no automatic mutation retries. If a response is lost, a change may have already
occurred; retrieve the current status before deciding whether to retry. The boundaries are intended for a local
control interface, not as a mass enrollment protocol for high data rates. The data path limits
remain separate.

<a id="verifikation"></a>

## Verification

Unit tests check registration without implicit rights, shared session limits for new clients,
disable and enable, invalid atomically rejected changes, revocation including pending challenges
and export after administrative resetting and normal rotation. Integration tests use data unix
socket and WSS next to the admin socket, all CLI routes, snapshot restart, file modes, other
peer UID, unknown/doppelte/oversized requests and protection of existing or exchanged socket
files. A real CLI server is terminated by SIGTERM; both of its own socket files are removed.

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked -p mynofy-client
cargo check --locked -p mynofy-protocol --no-default-features
```

No new registry dependency, so the remaining deferred prototype steps were not brought forward.

<a id="content-speichern"></a>

## Save content

`mynofy admin content save --socket PATH` sends `{"version":1,"command":{"op":"content_save"}}`. The answer `{"queued":true,"revision":N}` confirms the recorded and lined RAM copy. Write/encryption then runs without core lock; check the conclusion with `mynofy admin content status TOPIC --socket PATH` on the basis of the saved revision. At most one save at the same time, otherwise `content_save_busy`; without configuration `content_storage_disabled`. The path comes exclusively from the CFG. There is no Admin-Load/Restore/Reload. [Establishment and borders](content-storage.md).

With `--topics-json '["jobs/status","machines/status"]'`, an atomic group can be specified. The
JSON array preserves commas within a name and is also available at `admin peers set`. `--topics
TOPIC,TOPIC` remains the short form for names without commas. The limit is 16 KiB for the fully
coded admin request without line break. Separate subgroups are separate commits.

Without selection, all persistent topics are backed up together. Interval saves capture mature,
altered interval topics. For a signed-in revision N must apply to each requested topic
`content_storage.topics[NAME].saved_revision >= N`. `save_failure` contains revision and
diagnosis of a still open error for this topic. The global `saved_revision` can increase by a
successful save of other topics. Void, duplicate, unknown or RAM selects
`invalid_content_topics`.

<a id="topic-speicher-aktivieren"></a>

## Activate topic memory

`mynofy admin topics storage TOPIC --mode manual|interval [--seconds N] [--encrypted true|false] --socket PATH` sends `topic_storage`. Only local RAM topics can be activated; switchback and other changes to persistent strategies are rejected online. The switch is initially only RAM state and only becomes permanent with successful content save. The content save captures the active messages with their original expiration times. Offline switchback, backup, restore and rekey: [Leitfaden](restart-and-storage.md).


For [peer and proxy operation](service-network.md), `service_links` includes
registered peer IDs, active channel resources and limited routes. Peer-/Routing-Mutation.

## Service-Peers

`peers list|set|delete|disable|enable` and
`peers revoke-certificate|revoked-certificates` extend the same separate
Admin-Socket. [commands, security effect and persistence](runtime-peers.md).

All completed responses use the same JSONL framing: one JSON object followed by LF and connection shutdown, including `security_storage_unsynced` errors. A transport interruption or deadline can still truncate a response; never interpret such a partial frame as a successful mutation confirmation.


Four admin commands can run at the same time or wait for their security confirmation. A total of
six connections are allowed. With four waiting commands, `status` can use the remaining
connections. On this socket, all connections share the input phase. Two incomplete requests can
occupy the remaining seats until the end of their reading phase. The read-in period is two
seconds from the start of processing. The same UID, input, time and rate limits apply to all
connections.
