<a id="offline-werkzeuge-und-gezielte-speicherabbrüche"></a>

# Offline tools and targeted memory abortions

The offline CLI offers:

```sh
mynofy storage help
mynofy storage check --config config.yaml
mynofy storage inspect --config config.yaml
mynofy storage cleanup --config config.yaml
mynofy storage backup --config config.yaml --out backups/content-001
mynofy storage restore --config config.yaml --from backups/content-001
mynofy storage rekey --config config.yaml
mynofy storage topic-ram --config config.yaml --topic jobs/status
```

Stop the service before running offline maintenance. The tools hold
exclusive content and security locks. Provide secrets through stdin.
Restore preserves the current security state and validates content before
writing. Switching a topic to RAM removes its persisted content through
the offline transition. [Restart and storage](restart-and-storage.md)

`inspect` reports each topic's storage mode, saved revision, file size
and active message count. Stored bytes include framing and encryption
overhead. The command does not print payloads or secrets.

`cleanup` first validates the full stored state and then removes only unreferenced internal hash and slot files and the fixed staging names `topic.tmp`, `state.tmp`, `references.tmp`. Active files, security, unknown files and external backup directories remain. Symlinks in the unreferenced internal namespace are removed themselves, not their destination. Re-cleaning is allowed. `topic-ram` can also be repeated after an already completed manifest commit to complete an interrupted file cleanup.

Incomplete command arguments are rejected before CFG-/Storage-access and Secret query. `storage
help` does not require configuration. The existing global rules for early rights removal apply
unchanged.

## test procedure

```sh
./scripts/test-storage-crashes.sh
```

Alternatively, a single group:

```sh
cargo test --locked -p mynofy-server storage_fault_empty_restore -- --nocapture
```

The tests are part of `cargo test --locked --workspace`. Six matrix tests start together **176
real child processes** with private test directories and known test credentials. A separate
worker test is marked as `ignored` in the normal run and is started exclusively by the matrix
with `--ignored --exact`.

The worker uses the same `Engine` -/ `OfflineStorage` writing paths as service and CLI. At a
selected step, he reports to the parent via stderr that the point has been reached. In the event
of termination, he waits there without a busy loop; only then does the parent terminate with **
SIGKILL ** and wait for the end of the process. There are no estimated sleeps to hit a writing
time. Limited deadlines detect missed breakpoints or hanging workers.

The breakpoints and part-writes are built-in with `cfg(test)` only. In the normal
Library-/CLI-Build no fault switches, environmental checks, waiting points or additional
part-writes are generated. No additional registry dependency and no own unsafe.

<a id="matrix-und-erwartungen"></a>

## Matrix and expectations

| area | Tested limits |
| --- | --- |
| Topic-Dateien | After Truncate, real half staging writing, full writing, file sync, rename and directory sync; first and second topic files each. |
| Manifest | Partial or complete Write, File Sync, vor/nach Rename, vor/nach Directory Sync. |
| Cleanup / Storage→RAM | Before and after removing old slots and before final directory sync; resuming cleanup. |
| New Restore target memory | Half reference file, reference rename and partially or fully written initialization marker before the first manifest. |
| I/O-errors | Simulate ENOSPC and EIO to Open, Write, File Sync, Rename and Manifest Directory Sync for Save, Backup, Restore and Rekey. |

Of the 176 cases, **109 SIGKILL are aborts**, **34 ENOSPC -** and **33 EIO injections**. The
main matrix uses a common Raw Key Secret with a plaintext topic and an encrypted topic.
Additional aborts use completely unencrypted content and password mode. Messages use real 4096
byte payloads.

After each case, the memory is reopened and checked:

- before the manifest commit, the previous full generation remains readable;
- after the already visible manifest rename, the new full generation can be read in this process
  termination;
- never a topic from the old and the other from the new generation;
- Content, payload length, stable message IDs and cursor positions remain correct;
- for the rekey, the valid current status accepts exactly the corresponding old or new secret;
- An interrupted backup without a manifest is not accepted as a full backup; the source remains
  byte-for-byte unchanged;
- Offline operations preserve the security snapshot and journal;
- When switching back to RAM, the other topic is retained; reopening/cleaning only removes
  unreferenced files.

<a id="geschlossene-wiederanlauflücken"></a>

## Closed restart gaps

An abort between the reference rename and the finished initialization marker previously blocked
a new restore to an empty target memory. The resumption is now narrowly limited: **no published
manifest**, valid Owner-/Checksum-binding of the already stored reference file and an empty or
valid prefix of the expected marker. The marker is written and synchronized completely; the
flood level is not recalculated or lowered. A damaged marker next to an existing manifest
remains an error.

If synchronization of the manifest directory fails after a rename, the
visible commit may still exist with uncertain durability. The running
service exposes `content_storage.requires_restart: true`, leaves affected
RAM changes dirty and rejects further saves with
`content_storage_requires_restart`. Offline tools report
`durability_uncertain` and require reopening before further maintenance.
Reopening confirms the visible directory state before any old slot can be
reused or removed. Injected ENOSPC and EIO cases verify that another failed
confirmation sync leaves data untouched.

Upon closing, one's own flock is explicitly released, so that a descriptor copy inherited for a
short time during a parallel process start does not further block the already terminated owner
object. A separate test keeps a descriptor copy open and checks for reopening.

In addition, Restore checks the full initialization-/Vergabestand of the backup before
changing the target data or the target high level, and a corrupted backup metadata file is not
interpreted as missing new status.

<a id="einordnung"></a>

## Classification

This checks real process terminations and simulated I/O-errors on the Linux file system of the
test environment. Power loss, Controller-/storage errors, network file systems and native
macOS runtime are therefore not validated. Even a successful rename without confirmed directory
sync is not a durability commitment in the case of power loss.
