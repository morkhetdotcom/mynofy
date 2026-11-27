<a id="neustarterkennung-und-offline-speicherwartung"></a>

# Restart detection and offline memory maintenance

<a id="service-id-und-neustartzähler"></a>

## Service ID and restart counter

`service.id` remains the permanent identity; there is no additional instance ID. The service loads the security memory, increases its `restart_counter: u32` with circulation and synchronizes the new state before a listener can accept clients. Failed starts may consume values. An error in this memory step cancels the start. Normal security changes remain asynchronous as before; the start counter is the explicitly agreed synchronous exception.

The counter is in the existing security snapshot. Older snapshots without a field start with 1
on the first updated start, followed by 4294967295, the 0. Clients check **inequality**, not
order of magnitude. After exactly 2^32 unobserved starts, the value remains ambiguous. A direct
embedded `Engine` without a security backend remains file-free and uses 0; for permanent restart
detection, configure the security backend. CLI requires an explicit, already initialized
security store for `serve`.

Auth-Welcome now includes:

```json
{"version":1,"type":"auth_welcome","request_id":"auth-proof","client_id":"client:ed25519:…","service_id":"local","restart_counter":12}
```

The admin status provides the same counter. Current owners also send a random 128-bit `boot_id`
in the `AuthWelcome`, which is new at each start and lies outside the stored snapshots. Stored
cursors use this identifier for continuation. A restorer can repeat the counter and still
receive a fresh boot ID. It does not appear in every message record or cursor. The eight-digit
cursor format remains unchanged. In the prototype, the field is optionally represented in order
to distinguish missing values of valid 0; the client explicitly requests its presence when
authenticating.

<a id="client-fortschritt-und-replay"></a>

## Client progress and replay

The managed SDK client `mynofy_client::progress::RestartClient` receives an authenticated
connection, its welcome and a `ClientProgress`. `connect_client_info` and
`connect_unix_client_info` respectively provide socket and welcome together. progress is tied to
service ID and stable client ID; the application keeps it for the next connection setup.

If the boot ID is changed, stored topic cursors are discarded. An already bound progress file
from an older version is reset on the first connection to a current owner. In the case of older
owners without a boot ID, the previous counter engages. Positions stored after a restore with an
older owner or older client are explicitly discarded. Applications with their own cursor
management compare the boot ID when setting up the connection. The first successful poll for
each topic starts without a previous cursor, even if the caller hands over an old, still
syntactically valid cursor. A rejected poll does not consume this replay. `poll(topic, limit)`
then uses the updated own progress. Legacy confirmation records are discarded during offline
migration. Messages may reappear; automatic republication, re-execution or exact one-time
processing are not derived from this.

Previous functions `connect_client`, `connect_unix_client`, `exchange` and `exchange_frame`
Direct socket APIs are left without managed progress. A wrapper is necessary so that no global
unrestricted state per Owner/Client It limits your progress to 10,000 topics.
connection or command retries.

The CLI can save its progress between calls:

```sh
mynofy client --socket run/mynofy.sock --key client.pk8 --owner local \
  --progress .local/client-progress.json
```

Alternatively, use the same option for the WSS client. JSONL -Requests and responses remain the
same. The progress file contains owner, client ID, counter, boot ID and cursor, no credentials.
Private regular file, no follower, exclusive lock, limited read-in and atomic synchronized
replacement protects the state. The generic JSONL - CLI sets the stored cursor for the poll on
the same owner only if the caller passes it as `after`. For this purpose, use the last
`next_cursor` of the topic. If `after` is missing, the poll starts at the beginning of the
available history. `RestartClient::poll` adds the stored cursor independently. Parallel use of
the same progress file is rejected. Also, a pending cursor reset remains stored over a client
termination. Changed poll progress writes the CLI only after a successful stdout flush;
unchanged responses do not trigger a file swapping; an atomic connection for processing by the
receiver does not arise. Without `--progress`, the previous direct CLI mode is preserved. The
adapter starts anyway without a cursor and remains without a permanent workflow checkpoint.

<a id="ram--storage-im-laufenden-dienst"></a>

## RAM → Storage in the current service

The global `content_storage` with directory and possibly common start secret must already be
configured at start. Even then, topics remain as standard RAM.

Using the separate local admin socket, a RAM topic can be activated:

```sh
mynofy admin topics storage jobs/status --mode manual --encrypted true \
  --socket run/mynofy-admin.sock

mynofy admin topics storage machines/status --mode interval --seconds 60 \
  --encrypted false --socket run/mynofy-admin.sock
```

The change takes effect immediately in the RAM. It does not start an implicit save; at
`interval` the configured next appointment. The first successful save saves all the still active
messages of the topic, including those created before the change. Without a successful save, the
runtime activation falls back on the previous CFG-/gespeicherte baseline. With a successful
save, the start restores the strategy stored in the manifest, even if the original CFG still
contains RAM.

```sh
mynofy admin content save --topics jobs/status --socket run/mynofy-admin.sock
mynofy admin status --socket run/mynofy-admin.sock
```

Only RAM → manual/interval is allowed online. Further Storage-/parameter changes and storage →
RAM are rejected online. During a running save, activation is also rejected to avoid conflicting
captured strategies. The normal save status still distinguishes queue acceptance and permanent
completion. There is no automatic CFG writeback.

Before restarting, `mynofy check-config --config config.yaml` checks the static `serve`
configuration including explicit security path and store ID for content. An older file with
implicit security path can still read `config-convert`. File bindings, journal, keys and
listeners are only checked for the respective operating commands.

## Offline-Werkzeuge

All of the following commands do not open listeners; they hold the content lock and the existing
security lock; they fail when the service is running; the security counter is not increased;
security status, key recalls and ACLs are not replaced from a content backup; existing security
journals are read as at startup; a torn-off last record may be shortened according to the
existing recovery rule.

<a id="prüfen"></a>

### Testing

```sh
mynofy storage check --config config.yaml
mynofy storage inspect --config config.yaml
mynofy storage cleanup --config config.yaml
```

`inspect` complements topic metadata without payloads; `cleanup` After successful testing, remove only orphaned internal slots and staging files. [gezielte Abbruchtests](storage-interruption-tests.md).

Read, decrypt and check manifest, all referenced topic images, Owner-/client bindings,
assignment metadata and active configured odds. Provides a small JSON summary. Missing,
defective or incompatible data are not treated as valid vacancy. Old individual owner
checkpoints from `MYNCP001` with `storage preview-legacy --config config.yaml` and then
transferred to topic format with `storage migrate-legacy --config config.yaml`.

### Backup

```sh
mynofy storage backup --config config.yaml --out backups/content-001
```

The destination must be a **new** directory; existing destinations are not overwritten; the last
stored content status is backed up: manifest, referenced topic files and at least the same
technical reference status. Unencrypted topics remain unencrypted; encrypted files retain their
existing envelope. The backup does not contain any security configuration, TLS files or client
private keys. An erroneously aborted destination may exist incompletely; without a valid
manifest, it is not a successful backup; the source remains unchanged.

### Restore

```sh
mynofy storage restore --config config.yaml --from backups/content-001
```

Recovers content of the same `service.id`. Current security remains relevant; missing required
identities are not created from the backup. There is no delivery-ID allocator or retained
confirmation state. A new empty content destination folder is allowed; an existing damaged
status is not tacitly treated as empty.

The restorer first writes into unreferenced topic slots and only then replaces the manifest.
Error before this commit allows the previous content status to be used. An already increased
technical status may remain as a secure gap in the event of a failed restore. In the case of a
sync error by manifest rename, the existing rule remains: durability unconfirmed, no further
write operations on the open store until restart.

If the backup uses an older secret of the same Passwort-/Key-Modus:

```sh
mynofy storage restore --config config.yaml --from backups/content-001 \
  --backup-secret stdin
```

First, the current destination secret, then the backup secret is read via stdin, and the
imported backup is stored when committing to the current destination secret, and different
global encryption modes are not an implicit migration path in this step.

<a id="gemeinsames-secret-wechseln"></a>

### Changing Secrets Together

```sh
mynofy storage rekey --config config.yaml
```

Read the current secret, the new secret and its confirmation only via stdin; terminal inputs are
hidden. Unequal confirmation cancels before writing. Provide three lines if stdin is redirected.
The mode `password_stdin` or `key_stdin` is preserved. All encrypted topics and the manifest
change together; unencrypted topics keep their strategy. The CFG is not changed. Old, separately
created backup directories keep their previous secret.

A failed commit before Manifest Rename makes the old Secret usable. Only after the joint commit
will the replaced slots be removed. If the post-Rename sync is unconfirmed, the actual visible
status must be checked upon reopening; no automatic relapse to plaintext or a second secret.

### Storage → RAM

First stop the service and set for the topic in the CFG `storage: {mode: ram}`.

```sh
mynofy storage topic-ram --config config.yaml --topic jobs/status
```

The command removes the topic from the stored content and deletes its internal topic files; no
automatic copy of the content of this topic is retained in the store. Other topics and security
are retained. A later service launch uses the RAM baseline for this topic. A CFG edit alone does
not reset an already stored storage strategy: it would not be distinguishable from the original
RAM baseline before successful online activation.

<a id="tests-und-verbleibende-grenzen"></a>

## Testing and remaining limits

Unit tests check the synchronous counter, write error, U32 turnaround, activation with and without a save,
pre-existing content, online rejection of the switchback, offline locks, atomic error cases of
Restore/Rekey, restore to an empty store, unchanged current security and never reset balances.
SDK tests check counter inequality including turnaround, multiple topics, unsuccessful polls,
binding to the owner and rejection of a welcome without counter. A real CLI process test covers
activation, save, hard quit, failed boot counter write before binding, cursor replay after
restart, backup, rekey, restore with old backup secret and offline RAM deletion.

Linux is tested here; native macOS-CI remains open. Physical power outages are not claimed.
Performance measurements and further memory optimizations follow as a separate next step.

Finally, Linux passed 176 workspace tests, the advanced feature test campaign (64,000 model
steps and eight campaigns with 16,384 cases each), format checking, Clippy with warnings as
errors, and the isolated client and protocol checks.

The subsequent [Offline curing and abort matrix](storage-interruption-tests.md) supplements
Inspect/Cleanup, resumable initial initialization and early save rejection in the case of
unconfirmed commit. The above test result describes the status before this extension; the
current test report is linked there.

A file-free embedded owner reports 0 without a permanent restart guarantee. `RestartClient`
discards cursors stored with 0 each welcome; even a persistent circulation to 0 leads to the
replay for safety.

Progress saves create a private, randomly named staging file exclusively in the checked parent directory. An existing `<progress-path>.tmp` neighbor is preserved. Ordinary failures clean up only the current save's own staging file; an abrupt kill can leave an orphaned file, which later saves do not adopt or truncate.

Both CLI JSONL client modes limit each input line to 64 KiB including its optional newline and terminate locally on overflow, even if stdin remains open. Explicit `expires: null` is invalid command input; omit `expires` to use owner policy. Backup and Admin file output report parent-directory synchronization failures rather than claiming a durable newly created name.

## Restore target integrity and retained producer bindings

A normal restore requires either a valid existing manifest (whose observed expiry cutoff is preserved) or a genuinely fresh destination marked as such. A previously committed store with a missing manifest is not a fresh destination: restore fails without replacing its files, because its previous cutoff is unknown. Recover the missing manifest from a consistent store copy, or deliberately choose a separate fresh destination. Restoring into a new destination does not preserve a lost destination's expiry history. Current security is never restored from content.

Offline `topic-ram` keeps the shared producer registry referenced by untouched topic files, including producers whose messages have expired. This preserves other topics' ability to reopen and bind messages to their original producers; it does not re-enable credentials or grant permissions.


The CLI reports `listening on …` only after tying the data lister and the configured admin
socket. A restart counter jump confirms the persistent preparation and can also occur in the
event of a later tying error.


The managed CLI progress file is limited to 4 MiB encoded JSON. Escaping long topic names can
reach this limit before 10,000 cursor entries. If a new cursor exceeds the budget, the CLI ends
the poll with an error before the stdout output and keeps the previous file.


<a id="rückwechsel-auf-eine-ältere-cli"></a>

### Change back to an older CLI

A current managed CLI stores the boot ID with the cursor. The CLI from v0.1.0 cannot load a file
with this field. When changing back, specify a new, previously unused `--progress` path and run
the first poll without `after`. The previous file can be preserved for the current CLI.

<a id="kapazitätsprofil-der-fortschrittsdatei"></a>

### Capacity profile of the progress file

A cursor change writes and synchronizes the entire register. The costs increase with the encoded
file and the latency of the local file system. The review measured for 20 individual pages in
the median 152 ms with a stored cursor and 268 ms with 6,001 cursors (about 1.5 MiB). Three
local release runs including process start and Broker-I/O. Measure this profile on the target
disk for many topics. Separate progress files can map separate topic sets. The CLI checks the
byte budget before stdout and publishes the file only after successful flush.
