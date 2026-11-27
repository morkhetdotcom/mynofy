# Security Journal

Message content and security state have separate lifecycles. Security changes apply immediately in RAM and enter an ordered append-only journal. By default, successful service security-mutation responses wait for disk synchronization. Explicit `sync: false` allows asynchronous confirmations and can lose unsynced changes on a crash.

## Durable security confirmations

The CLI service requires an explicit `security_storage.directory` and an initialized store. The optional `sync` field defaults to `true`. This example shows the effective default:

```yaml
security_storage:
  directory: ./security
  sync: true
```

Only set `sync: false` to opt out of durable confirmation.

`sync` defaults to `true`. With the default, successful security mutation responses on
the Admin socket and successful client key-rotation commits wait for the journal sequence to be synchronized to disk. This includes client/key/ACL changes, peer policy changes and topic security resets. It does not make message publications or topic content durable; those retain their separate storage policy.

RAM changes and session invalidations still happen immediately. The service waits outside the Engine mutex, on a blocking worker, with a five-second fence deadline; other clients and Admin status reads can continue. The Admin connection itself also has a bounded deadline. If synchronization fails, Admin returns `security_storage_unsynced` when its deadline permits a response. The change is already applied in RAM and may become durable later. Inspect status instead of blindly retrying. A failed rotation confirmation closes the client connection, preserving an unknown commit outcome; retain both keys until reconciled.

Admin status exposes `security_storage.sync`, `submitted_sequence`, `durable_sequence` and `last_error`. The asynchronous journal and retry mechanism are the same in both modes; `sync` changes the service's success-confirmation policy. Embedded synchronous Engine APIs still modify RAM and enqueue records; only the service transports perform the asynchronous confirmation wait.

<a id="einrichtung"></a>

## Setup

For the CLI service, explicitly configure `security_storage.directory` in YAML or JSON.

```json
"security_storage": {
  "directory": "/var/lib/mynofy/security",
  "queue_capacity": 128,
  "compact_after_records": 4096
}
```

Relative paths resolve against the configuration directory. Run `mynofy security-init --config FILE` once as the service user. The initializer stages the store in the same parent directory and publishes it after syncing its components. A retry discards recognized staging leftovers and rebuilds from the current configuration. Unexpected files and an already initialized store cause initialization to fail. The service user needs write access to the private parent directory during setup. Apply the intended operating permissions after initialization.

When upgrading, point to the existing store. Older installations with an implicit path usually stored it at `<config-filename>.security`. To recover after a loss, stop the service and restore the most recent consistent security store for the same owner. If using an older backup, reconcile subsequent revocations and ACL changes before accepting clients.

`serve` requires a nonempty, initialized store for the configured owner.
The parent directory must exist, and the store directory must be private and owned by the service UID. Store files are regular files with mode `0600` and a single hard link. An exclusive file lock prevents two instances from opening the same store. The file backend uses Unix APIs through the `rustix` crate.

Embedded callers can construct a `Config` with `security_storage: None` to use the core without a writer or store. `Config::load` resolves a storage path even when the field is absent or null. Embedded network listeners also require an initialized owner store. Use `transport::prepare_volatile` for a listener where both content and security are kept in RAM. `check-config` resolves the path without opening the journal or lock.

## Security state and baseline

The store records client registrations, stable identities, display names and disablement, registered public keys and their expiry, ACL replacements for exact local topics, peer policy, and topic security resets. A completed rotation stores the full resulting client credential set in a single entry.

Private keys, shared secrets, challenges, sessions, in-progress rotations, rate limiters and messages stay outside the store. Compaction retains the resulting security state and discards earlier changes.

On the first start, the configuration provides the baseline for clients, local topic ACLs and peer policy. The snapshot and journal become authoritative after initialization. Runtime peer allowlists, neighbor disablement and certificate revocations are managed through the local Admin API. Listener settings, TLS material, the upstream endpoint and static routes remain startup configuration. [Runtime peer management](runtime-peers.md)

A nonempty saved ACL referencing a missing local topic prevents startup, except for the dynamic-topic case described below. Content recovery creates neither clients nor grants. Conflicting names or owners and invalid references cause recovery to fail.

An Admin export is a manually requested configuration snapshot and includes the resolved security path. Set a separate security path before starting an independent copy. Resetting a security store is an explicit offline operation and can restore older keys and grants from the configuration.

## Runtime, failures and status

A dedicated writer thread processes changes in core order. Each mutation reserves a slot before modifying RAM. The queue limit covers records being written or retained for retry. The default is 128 and the allowed range is 1–4096. A full queue returns `security_storage_busy` before any security mutation. An unavailable writer returns `security_storage_unavailable`. An unsuccessful rotation commit can consume its one-time exchange. A later attempt starts with a new rotation ID. Data operations have no journal work on their hot path.

The writer batches up to 32 changes already waiting in the queue. It advances the durable sequence after successful `sync_all`. On failure it retains the batch, reports the error on stderr and in Admin status, and retries after 100 ms. It truncates a partially written batch back to its original offset before retrying. Compaction failures suspend further appends to keep journal growth bounded.

`mynofy admin status --socket PATH` includes `security_storage`:

| Field | Meaning |
| --- | --- |
| `enabled` | A security writer is configured. |
| `submitted_sequence` | Latest security change applied in RAM and queued. |
| `durable_sequence` | Latest journal entry successfully synchronized. |
| `snapshot_sequence` | Sequence of the latest completed checkpoint. |
| `pending_records` | Records reserved, queued or currently being written. |
| `last_error` | Current write or compaction error, or `null`. |

Normal shutdown drains the queue and stops the writer. If a storage error persists, the service reports unsaved state and exits. Operating system I/O can delay the writer or shutdown. SIGKILL can lose unsynchronized changes. Durability after `sync_all` depends on the filesystem and storage device honoring synchronization.

## File format and recovery

Local format v1 is independent of the message wire protocol and RAM record layout. The store contains `lock` (the initialization marker and process lock), `snapshot` and `journal`. Compaction uses `snapshot.tmp` as a temporary file.

Each frame has an 80-byte header containing `MNS1` (4 bytes), payload length (u32 LE), sequence (u64 LE), a SHA-256 digest of those first 16 bytes, and a SHA-256 digest of the payload. A versioned JSON payload follows. The header digest validates the length before allocation or acceptance of an incomplete payload. Checksums detect corruption. File permissions restrict writes to the store.

Journal payloads are limited to 16 KiB and snapshot payloads to 32 MiB. Serialization stops at the limit. A snapshot records the owner, sequence and resulting security state. Journal payloads include `version: 1`, the owner ID and a security change.

On startup, the service validates the snapshot and replays the journal in order. New records must continue the sequence without gaps. Recovery recognizes an incomplete final frame and can discard it after validating the accepted prefix. A complete frame with a bad digest, unsupported version or wrong sequence causes startup to fail. So do a missing initialized store, a corrupt snapshot or an owner mismatch. Records already covered by the snapshot are validated and skipped during replay.

The service creates a checkpoint after recovery and before accepting connections. During operation it checkpoints after 4,096 changes since the last checkpoint by default (configurable from 1 through 1,000,000) or at 8 MiB of journal data. At most one batch of up to 32 bounded records may cross that threshold. Recovery rejects journal files larger than 16 MiB.

Compaction writes and synchronizes the new snapshot, atomically renames it, and synchronizes its directory before truncating and synchronizing the journal. A crash between these steps leaves either the old snapshot with its journal or the new snapshot with previously covered journal records.

## Verification

Unit tests cover sequences, incomplete final frames, corrupt headers and payloads, versions, snapshot checksums, missing baselines, locks, filesystem permissions, symlinks, size limits and retries after write failures. Core tests cover mutations including rotation, immediate RAM updates while compaction is blocked, and rejection before mutation when the queue is full. A CLI process test uses SIGKILL after durable confirmation and verifies that permissions survive restart with an empty message store. Separate tests construct interrupted on-disk states to check recovery boundaries.

The [content storage backend](content-storage.md) separately handles optional encryption and startup restore. Security retains its own store.

## Synchronized restart counter

The security snapshot includes a wrapping `restart_counter: u32`, which defaults to zero for older snapshots. After recovery, startup increments and durably checkpoints the counter before accepting clients. The authentication welcome and Admin status expose it. Offline content tools hold the security lock and read permissions without incrementing the counter or replacing security configuration.

## ACL lifecycle for dynamic topics

[Runtime topic management](runtime-topics.md) uses the `topic_reset` record.
Creating a topic clears existing ACLs for the name and marks it in `transient_topics`. Deletion clears the ACLs and marker. The topic definition lives in RAM or content storage. During recovery, ACLs for a marked topic are discarded if neither the configuration nor content recovery restores it. Missing unmarked local ACL targets cause recovery to fail. Recreated topics start with empty ACLs. A stored dynamic topic is prepared before security recovery and receives its saved permissions.

Older snapshots without `transient_topics` can be loaded. Versions that predate this record and marker cannot read a store that already uses them.

## Oversized-record admission

The concrete owner/change envelope is serialized through the same bounded 16-KiB record encoder before it can enter the write queue. A runtime peer update submits this validated change before replacing its RAM policy. An admin request that fits the 16-KiB input limit can therefore still return `security_record_too_large`; input and journal envelopes have different overhead. That rejection consumes no sequence or queue entry and does not block subsequent valid security changes. Filesystem failures retain asynchronous retry behavior.

## Total-state admission

`security_snapshot_too_large` rejects a mutation before RAM changes when the
resulting security state would exceed its recoverable snapshot budget. Accounting includes queued changes and reserves space for the owner/envelope and counter growth; deletions release capacity. Individual records retain their separate 16-KiB check. Normal disk I/O remains asynchronous. See the [storage and restart guide](restart-and-storage.md).


### Recovery floor and planned shutdown

A deployment may set `security_storage.minimum_sequence` from a trusted record of the last acceptable durable security sequence. Startup and offline content maintenance reject a replay below that value. Keep the reference outside the restored security volume. After restoring an older generation, review ACLs and revocations before admitting clients.

The Admin listener reserves time to send a structured failure after a durability wait. A supervised shutdown reports a drain that lasts over ten seconds. The process continues to hold its store locks while I/O finishes.


`check-config` validates configuration statically. A positive
`minimum_sequence` requires a qualifying recovered store, so
`security-init` rejects that setting before writing a new store.
Normal SIGTERM stops acceptance, closes sessions and drains the writers. If the accepted security sequence exceeds the durable sequence after drain, `serve` exits with an error and logs `security_drain_failed` with both sequence numbers. Embedded owners can pass a shutdown future to `run_prepared_until` or `run_unix_prepared_until` to receive the same result.


Recovery validates the replay prefix against `minimum_sequence`. On service startup, compaction follows successful validation of both content and security. An earlier rejection preserves the journal bytes. Explicit offline maintenance can repair an incomplete tail on an accepted prefix.
