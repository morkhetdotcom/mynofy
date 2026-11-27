<a id="topic-speicherung-start-restore-und-gemeinsames-secret"></a>

# Topic Storage, Start-Restore and Common Secret

RAM remains the default. Each local topic owner sets the memory strategy of his topic in the
CFG. Security changes will continue to be automatically stored in the
[Security-Journal](security-journal.md) regardless. Publish confirms RAM assumption; Poll reads
the RAM inventory; a successful save receives the recorded status for a restart.

<a id="konfiguration"></a>

## Configuration

Add global settings to the existing CFG service:

```yaml
content_storage:
  directory: content
  store_id: "2b3167d768a1e5fd9cd12391c4df9707"
  encryption: password_stdin
  max_snapshot_bytes: 134217728
```

For the respective entry in `topics` add:

```yaml
# Volatile communication; even without storage specifying the default.
- name: agents/communication
  # owner_id, policy and acl configure as before.
  storage:
    mode: ram

# Consciously secured state, with file encryption.
- name: jobs/status
  storage:
    mode: manual
    encrypted: true

# Periodic backup for changes.
- name: machines/status
  storage:
    mode: interval
    seconds: 60
    encrypted: false
```

The topic examples show only the new fields; `owner_id`, TTL - `policy` and `acl` remain
necessary. Memory rules are owner configuration and are not changed by producers or topics ACL.

| Topic-Strategie | behavior |
| --- | --- |
| `ram` | Do not store messages or topic headstands. |
| `manual` | Only by explicit admin save. |
| `interval` | Regularly save when the topic condition changes; manual save is also possible. |

`seconds` allows 1–2.592,000 seconds. `encrypted` is false by default for stored topics; an encrypted topic requires global `password_stdin` or `key_stdin`. Without `content_storage`, only RAM topics are allowed. A directory alone does not make a topic persistent. RAM →Storage can be activated via the administrator API; the change is only permanently made by a successful save. Storage→ RAM takes place exclusively via the offline tool. [operation](restart-and-storage.md).

`directory` is absolute or relative to CFG. The parent directory must exist and be accessible to the final service user. `mynofy content-init --config config.yaml` sets up the volume with mode 0700 and files with mode 0600. The files and directory must belong to the service UID. Symlinks, additional hardlinks and third-party file owners are rejected. An exclusive lock prevents two instances in the same directory. Content and Security require separate directories.

Create your own `store_id` with `openssl rand -hex 16`. Run `mynofy content-init --config
config.yaml` once when the service is stopped. Point to the already existing, checked volume. At
start, mynofy checks the configured identity. A missing or incorrect volume leads to
cancellation. Use the previous Store ID for a restore to a new volume and explicitly initialize
the destination before the restore.

<a id="ein-gemeinsames-start-secret"></a>

## A common start secret

| Globales `encryption` | Enter at start |
| --- | --- |
| `none` (Default) | No input; only unencrypted topic images. |
| `password_stdin` | A password line from stdin. |
| `key_stdin` | Standard canonical base64 of a symmetrical 32-byte key from stdin. |

```sh
mynofy serve --config config.yaml
```

The secret is read exactly once and used for all encrypted topics. The owner metadata in the
manifest is also encrypted when the global encryption mode is activated, even if individual
topic images remain unencrypted.

At the terminal, the input is hidden; normal returns and input errors restore the echo. With
redirected stdin, a limited line is read without prompt. Only LF or CRLF is removed; password
spaces remain. EOF without line break is allowed. Empty inputs and inputs over 4096 bytes are
rejected.

Secrets are neither in CFG/argv/env nor in the admin export or status. `check-config`,
conversion and admin commands do not query them. The secret remains in a `Zeroizing` buffer for
saves in the running process. Content is decrypted in the RAM during operation; this option
protects stored files. The security journal remains separate.

A topic image carries its actual stored encryption mode in the manifest. If the encryption of an
explicitly persistently configured topic deviates, the service refuses to start. Stop the
service, set the desired value in the configuration and run `mynofy storage topic-encryption
--config FILE --topic TOPIC`. The command reads the existing image according to its stored mode
and replaces the topic file atomically via the manifest. Restore rejects conflicts with the
target configuration before writing. Change of the secret explicitly offline with `storage
rekey`; the global secret mode remains the same. No implicit rekey or plaintext fallback.

<a id="einzelne-topics-oder-gruppen-speichern"></a>

## Save individual topics or groups

```sh
# Capture all topics together with manual or interval.
mynofy admin content save --socket run/mynofy-admin.sock

# Collect only these topics together.
mynofy admin content save --topics jobs/status,machines/status --socket run/mynofy-admin.sock

mynofy admin status --socket run/mynofy-admin.sock
```

A save captures all selected topics under the same owner synchronization. Related messages and
payloads are copied. RAM Topics and unselected topic contents are not copied. The capture
process requires CPU and additional memory proportional to the selected inventory; it is not a
constant real-time operation.

The answer `{"queued":true,"revision":N}` only confirms the enrolled copy. Then frame formation,
hashing, encryption and Datei-I/O run on a separate writer thread without engine lock. At most
one save is pending; further requests result `content_save_busy`. Without content memory, the
result is `content_storage_disabled`; unknown, duplicate, empty or RAM topic selections result
`invalid_content_topics`.

Interval saves select only mature, modified interval topics. They do not store current messages
of a manual topic. Unselected stored topic images remain unchanged. Appointments use monotonous
runtime; maintenance can delay them. After a common spelling error, the state remains dirty and
can be backed up manually at the next interval. There is no unlimited Save-/Retry-Queue.

Independently stored topics can have different time periods. For a coherent workflow, the
affected topics are explicitly secured together. This creates a common technical acquisition
time; this does not result in an atomic technical transaction between publish and processing.

<a id="status-und-abschluss"></a>

## Status and completion

`status.content_storage` contains:

| Field | Meaning |
| --- | --- |
| `enabled` / `encrypted` | Memory set up / common secret mode active. |
| `loaded` | At the start, a manifest or older checkpoint was loaded. |
| `revision` | Current content change status of persistent topics. |
| `saved_revision` | Revision of the last successfully saved save order. |
| `dirty` | At least one persistent topic has not yet saved changes. |
| `save_in_progress` | Save placed or in processing. |
| `requires_restart` | Unconfirmed manifest commit; further saves rejected until restart. |
| `last_save_time_ms` / `last_error` | Last time of completion / current error or zero. |
| `topics` | Per persistent topic strategy, `revision`, `saved_revision`, `dirty`, `loaded` and a possibly open `save_failure`. |

For a save of revision N under `topics`, check each selected position. Your `saved_revision`
must be at least N. `save_failure` contains revision and errors of the last failed save order
for this topic. A successful save removes the errors of its selected topics. The global
`saved_revision` and `last_error` describe the last successful or last save and are not
sufficient to confirm a previous selective order. New changes during a save remain dirty. RAM
changes do not trigger save work.

Regular quit waits for already accepted saves, but does not start an implicit additional save.
SIGKILL may lose ongoing and post-save changes. Operating system I/O may delay quitting.

<a id="atomare-dateistruktur-und-grenzen"></a>

## Atomic file structure and boundaries

`state.bin` is the versioned manifesto `MYNTOP01`: common owner metadata plus a limited topic table with topic name, storage strategy, revision, file size, slot and SHA -256 of the stored image. JSON The owner metadata uses the previous binary checkpoint frame without any messages/Topics.

Each topic receives its own binary `MYNCP001` image with messages, policies, headstand. It
embeds the [Common message layout](binary-message-layout.md). Only actually required producer
bindings are additionally in the topic image; the common identity table is in the manifest.
Relative memory addresses and slots are adjusted when assembling or merging, without
JSON-/Protobuf-conversion of the messages or event values. The RAM arenas remain in common so
as not to load small topics with their own reserved pages.

File names are created from SHA -256 of the topic name and one of two slots; topic names are not
used as paths. The writer writes and synchronizes the unreferenced topic slots first. Only when
all selected images are finished does he replace `state.bin` atomically and synchronize the
directory. A cancelled task before this commit leaves all previous manifest references valid.
After successful commit, replaced slots are removed. Errors before the commit clean the created
uncommitted slots. After full start validation, orphaned internal topic slots are removed.

If the directory sync fails **after** the manifest name, the new valid manifest can already be
visible, but its durability is unconfirmed. Further saves are denied until restart so that no
possibly referenced slot is overwritten. This is reported as a memory error.

`max_snapshot_bytes` limits the sum of current manifest and all referenced topic files including encryption envelopes; allowed are 64 KiB to 512 MiB, Default 128 MiB. During a save, old and prepared files exist side by side. RAM copy, a topic image and serialization-/encryption buffers require additional memory. Capture and file parser are limited; new registry dependencies were not introduced.

Encrypted files use AES -256- GCM with authenticated 64-byte header and 16-byte tag, fresher
96-bit nonce per Datei/Save. Password mode uses a random 128-bit salt per store key; new files
and the manifest share this salt. The Raw key has 256 bits; Password mode uses PBKDF2 - HMAC -
SHA256 with 600,000 iterations. The expensive password derivation is done once when the store is
opened before serving; the prepared key is reused for saves. Old files with independent salts
remain readable and require their original derivation when loaded until they are rewritten.
Format and 600,000 rounds remain unchanged; Offline rekey generates a new salt. No plaintext
temporary files are written for encrypted images. Serialized plaintext buffers, secrets and
derived keys are nullified when shared normally. Checksums of unencrypted files detect damage,
but do not authenticate an attacker with write privileges.

The implementation uses the existing
[AWS-LC-AEAD-Provider](https://docs.rs/aws-lc-rs/latest/aws_lc_rs/aead/) and orients them.
PBKDF2 -Arbeitsfaktor an
[OWASP](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html). None
FIPS -Zertifizierungszusage.

<a id="vollständiger-start-restore"></a>

## Full Start-Restore

The entire Load/Decrypt/Validate/index construction runs before listener binding: Dismantle rights →
CFG and read one-time secret → Manifest and **fully check/decrypt all referenced topic files** →
Assign current security → Build up active inventory and indices → Open listeners. False secret,
fehlende/defekte referenced file, false owner, unknown identity or active odds cancel the start.
No partial operation start and no automatic vacancy as error caseback.

Stored storage strategies take precedence over the original RAM baseline in the CFG for a
successfully stored online change to survive a reboot. A CFG edit alone is not a storage→RAM
change. The explicit offline command removes the stored topic content and its files; the CFG
baseline must provide RAM. A missing topic definition can be restored from the image without
inventing rights. Current security remains relevant for clients, locks, keys and ACLs. [Restart
and offline maintenance](restart-and-storage.md).

Save messages/Payloads/IDs/absolute times, Topic-Policies/-heads, Producer positions,
shared message transfers. Sessions, challenges, incomplete rotations and rate limiters are not
restored. Expired messages are removed before client acceptance; the service life is not
extended. The saved owner time maximum prevents resuscitation when the clock bounces back.

Topic cursors retain stored headstands. A poll cursor recognized as invalid starts again at the
active beginning with the same Seiten-/Byte-Limits. old matching eight-digit values remain
ambiguous. Producer list cursors get a new process generation.

**No Load/Restore/Reload or reload during operation.** Subsequent file changes are not read in.
The writer only uses the manifest table loaded at startup and the rows of RAM copies; he does
not read topic files.

<a id="vergabestand-und-backups"></a>

## Assignment status and backups

The retired `references.bin` delivery allocator is no longer created, advanced or required. Existing MYNCP001 confirmation sections are validated and discarded during offline migration; new snapshots contain none. See [upgrade notes](message-client-confirmation.md).

Back up the manifest and its referenced topic files together while no save is running. After restore, the owner reserves message ID blocks on demand from the saved reservation position and checks for collisions with active messages. Applications that require globally unique event identities across backups must supply their own.

<a id="wechsel-vom-bisherigen-owner-checkpoint"></a>

## Change from the previous owner-checkpoint

Remove the previous global `content_storage.save_interval_seconds`. Remove this field from the
CFG and set desired strategies explicitly to the topics; without specifying, the respective
topic remains RAM. An old individual `MYNCP001` checkpoint is checked offline with `storage
preview-legacy` and transferred to topic files and manifest with `storage migrate-legacy`, with
the same global secret mode. All stored topics need an explicit CFG definition and at least one
persistent strategy. Otherwise, the start will stop without replacing the old checkpoint with
empty content. Topics configured as RAM will not be adopted. The new manifest commit will
receive the old stored state until the change is successful.

<a id="verifikation"></a>

## Verification

Unit and process tests check all three secret modes, concealed terminal echo, SIGKILL restore,
stateless polling, current ACLs, expiry/content limits, typographical errors, independent
manuelle/Intervall-Saves, group rollback, RAM-Default/-rotation, mixed file encryption,
corrupted/missing topic files, strict configuration and the adoption of older checkpoints.
Native macOS runtime check remains noted for GitHub CI. New performance values or physical power
failure measurements are not claimed.

Targeted process terminations, ENOSPC/EIO and the offline commands `inspect` / `cleanup`:
[storage-Abbruchtests](storage-interruption-tests.md).

## Content working-memory admission

`limits.max_content_working_bytes` defaults to 268435456 bytes (256 MiB), accepts
nonnegative byte counts representable on the host, and controls conservative runtime save admission separately from `max_active_message_bytes` and the disk limit `max_snapshot_bytes`. Zero rejects saves. The estimate accounts for six times allocated record/blob/type storage, pair/index and registry allowances, plus 1 MiB of fixed headroom. It currently considers the whole engine even for a selected-topic save. A rejection returns `content_memory_budget_exceeded` before capture and releases the save slot. `content_storage.working_memory_reserved_bytes` in admin status reports the admitted estimate while capture/writing is in progress, then returns to zero.

Only one content capture/write runs per engine. Broad saves pin initialized record and payload blocks under the owner lock, together with producer counters, topic heads/policies, expiry cutoff and ID reservations. The worker selects and compacts this immutable epoch before writing. A later live mutation copies only its affected shared block; an ordinary unique write performs no atomic referencecount operation. When the last snapshot is gone, a subsequent write recovers the original unique buffer without copying it.

For smaller selections, the existing topic indexes capture only selected records under the lock, without pinning unrelated RAM payload pages. Saves with no selected records do not pin empty retained arenas. Unselected persistent-topic metadata remains bound to the same epoch; RAM-only content is never written. No wire or disk format changes.

A queued save is not a completed save: compaction/size validation, encryption and I/O may fail on the worker. Such failures preserve the previous manifest, keep changes dirty and release the save slot/working-memory reservation. Check status for completion. A compatible single-topic image is borrowed by the writer rather than copied again. Capture still has bounded-by-data metadata and copy-on-write costs; it is not lock-free. This setting is an admission estimate, not a hard allocator or process RSS cap. Startup/offline loading, transport buffers, normal live-message growth and allocator retention require separate headroom. Per-response transport limits remain independent. See the [storage and restart guide](restart-and-storage.md).

## Security dependency of a content save

Each save captures the submitted security-journal sequence with its content image. The background content writer waits outside the core lock until that sequence is durable before writing the checkpoint. A security error, stopped writer or five-second dependency timeout fails the save, preserves the previous committed content and leaves the captured changes dirty. Inspect content status and retry once security storage recovers. RAM security changes remain asynchronous and immediately effective. See [security journal guide](security-journal.md).

Reducing a topic's TTL policy affects future publishes. Previously accepted messages retain their original expiry across save and restart; checkpoint validation applies the protocol-wide lifetime bound to those records.

SIGINT or SIGTERM during interactive secret entry cancels the prompt with a controlled error and restores the original terminal settings before exit. The same applies to offline storage prompts. Partial terminal input is discarded on aborted entry; successful multi-line piped restore/rekey input is preserved. A catchable prompt interruption does not start listeners. SIGKILL cannot run this cleanup. See [restart and storage guide](restart-and-storage.md).

## Detecting a missing manifest

The content lock file records whether the store is fresh or has committed a manifest. A missing manifest in an initialized store aborts startup and preserves remaining topic files. A genuinely fresh store can still recover from partial files left by an interrupted first save. Legacy stores with a valid manifest are upgraded when opened. Offline restore retains the maximum saved owner-time cutoff of the destination and backup, together with the current time, so a clock rollback cannot revive a record already expired in the destination's saved history. See [restart and storage guide](restart-and-storage.md).

## Aggregate size of the next generation

`max_snapshot_bytes` limits the encoded files of the next committed generation,
including its manifest. A multi-topic save counts unchanged files plus the new versions of all selected topics; replaced old versions are excluded together, so growing one selected topic and shrinking another is independent of selection order. Staging still needs physical disk space for old and new slots until commit.

Live RAM-topic names and policies do not consume the encoded snapshot budget. Projection limits bound the image itself; the bounded encoder and aggregate writer count actual serialized controls and files. The separate working-memory admission may conservatively include the entire live owner. Check save completion and errors through status; an accepted queue request is not a commit receipt.

After the manifest commits, failed deletion of old topic slots appears as `cleanup_error` in content status. The writer retries cleanup after one second using the latest committed manifest as its keep set. Save revision and save errors continue to report the commit itself.

Selective saves reserve working memory from the selected records and shared metadata. Larger selections can pin the record chunks and blob pages they reference, then project an independent compact image on the writer. The reservation includes pinned blocks and potential detached live copies. Small selections and selections whose pins would exceed the budget use a compact capture directly. A broad capture that exceeds the working budget can fall back to selective capture. Neither path includes unrelated RAM messages in the persisted generation.


Autosave retry is configured under `content_storage.autosave_retry`. Its defaults are `enabled: true`, `initial_delay_seconds: 1` and `max_delay_seconds: 60`. A dirty scheduled topic whose save failed is retried with doubling delay up to the configured maximum. The regular interval resumes after success. An uncertain manifest pauses saves and reports `requires_restart`.

Content status reports `last_writer_duration_us` separately from capture. It includes the writer’s security fence, projection and file operations.

The writer synchronizes each staged topic file before renaming it into an inactive slot. After all selected topic slots are staged, it synchronizes the directory once before replacing the manifest. The manifest replacement has its own durability barrier. A failure at the staging barrier leaves the previous manifest and its referenced slots intact.


After a capacity failure, automatic saves retry the affected selections separately. Topic status exposes `autosave_isolated` during these retries. Explicit multi-topic saves keep their atomic commit behavior.

Legacy `MYNCP001` data requires offline conversion. Run `mynofy storage preview-legacy --config FILE` to list the saved topics and record counts. The preview holds offline locks and opens both stores read-only. If it encounters an incomplete final security journal record, it reports it and leaves the source files intact. The explicit migration command uses normal security recovery, which may remove an incomplete final record. Configure every saved topic as locally persistent, then run `mynofy storage migrate-legacy --config FILE`. The migration validates security bindings and active limits before committing the new manifest.


A backup receives the Store ID only after the complete manifest has been permanently written.
The incomplete backup path remains locked for the regular service launch. Already set up,
previously empty volumes from older builds need a one-time `content-init` on the tested target
volume. Existing full generations remain readable. After an offline operation, `commit=durable
cleanup=pending` indicates an already effective change with remaining files. `storage cleanup`
cleans up these files. Rekey already applies the new secret.

When the service stops regularly, the content writer is emptied. If the last saved message has
failed or the persistence of the manifest is unclear, the service reports
`content_drain_failed`. Without a saved message, the configured storage strategy applies until
the last confirmed status.


Restore opens the backup source with write-free file access and an exclusive lock, the source
needs the marker and manifest of a completed generation, and the goal remains writable.

When starting together, content and security are checked under their store locks. Only after
successful content recovery in RAM is the security restart counter confirmed, then content
cleans up unreferenced files. A truncated journal end is removed at the start of the service
only during this compaction. Content confirms the existing file system and, if necessary, the
visible manifest marker for the crash recovery. For the diagnosis of immutable images, keep
originals read-only and use a working copy.
