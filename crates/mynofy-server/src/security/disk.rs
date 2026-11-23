use super::*;
use crate::crypto;
use rustix::fs::{CWD, FlockOperation, Mode, OFlags, flock, openat};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::Path,
    time::Duration,
};

const HEADER: usize = 80;
const MAX_RECORD: usize = 16 * 1024;
pub(super) const MAX_SNAPSHOT: usize = 32 * 1024 * 1024;
const COMPACT_BYTES: u64 = 8 * 1024 * 1024;
const MAGIC: &[u8; 4] = b"MNS1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot<T> {
    version: u8,
    owner: String,
    sequence: u64,
    state: T,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record<T, O = String> {
    version: u8,
    owner: O,
    change: T,
}

pub(super) struct Store {
    directory: File,
    journal: File,
    lock: File,
    initialized: bool,
    owner: String,
    pub state: State,
    pub sequence: u64,
    snapshot_sequence: u64,
    length: u64,
}
// Release the owner's flock immediately, even if a concurrent process spawn briefly
// inherited a duplicate before exec closes its CLOEXEC descriptors.
impl Drop for Store {
    fn drop(&mut self) {
        let _ = flock(&self.lock, FlockOperation::Unlock);
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn file(directory: &File, name: &str, create: bool) -> io::Result<File> {
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file: File = openat(
        directory,
        name,
        if create {
            flags | OFlags::CREATE
        } else {
            flags
        },
        Mode::RUSR | Mode::WUSR,
    )?
    .into();
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid(
            "Security files must be private regular files owned by the service UID",
        ));
    }
    Ok(file)
}
// Bound serialization before allocation can grow beyond the configured format limits.
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(invalid("Security record/snapshot size limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(sequence: u64, value: &impl Serialize, limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Bounded {
        bytes: vec![0; HEADER],
        limit: limit + HEADER,
    };
    serde_json::to_writer(&mut output, value).map_err(io::Error::other)?;
    let mut record = output.bytes;
    let length = record.len() - HEADER;
    record[..4].copy_from_slice(MAGIC);
    record[4..8].copy_from_slice(&(length as u32).to_le_bytes());
    record[8..16].copy_from_slice(&sequence.to_le_bytes());
    let header_hash = crypto::sha256(&record[..16]);
    let payload_hash = crypto::sha256(&record[HEADER..]);
    record[16..48].copy_from_slice(&header_hash);
    record[48..80].copy_from_slice(&payload_hash);
    Ok(record)
}
pub(super) fn validate_change(owner: &str, change: &Change) -> io::Result<()> {
    encode(
        0,
        &Record {
            version: 1,
            owner,
            change,
        },
        MAX_RECORD,
    )
    .map(|_| ())
}
enum Frame {
    End,
    Torn,
    Complete(u64, Vec<u8>),
}
fn read_frame(file: &mut File, limit: usize) -> io::Result<Frame> {
    let mut header = [0; HEADER];
    let mut read = 0;
    while read < HEADER {
        match file.read(&mut header[read..]) {
            Ok(0) => return Ok(if read == 0 { Frame::End } else { Frame::Torn }),
            Ok(n) => read += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
            Err(error) => return Err(error),
        }
    }
    if &header[..4] != MAGIC || header[16..48] != crypto::sha256(&header[..16]) {
        return Err(invalid("Security frame header checksum/version mismatch"));
    }
    let length = u32::from_le_bytes(
        header[4..8]
            .try_into()
            .map_err(|_| invalid("Invalid length"))?,
    ) as usize;
    if length > limit {
        return Err(invalid("Security frame exceeds size limit"));
    }
    let mut payload = vec![0; length];
    match file.read_exact(&mut payload) {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(Frame::Torn),
        Err(error) => return Err(error),
        Ok(()) => (),
    }
    if header[48..80] != crypto::sha256(&payload) {
        return Err(invalid("Security payload checksum mismatch"));
    }
    let sequence = u64::from_le_bytes(
        header[8..16]
            .try_into()
            .map_err(|_| invalid("Invalid sequence"))?,
    );
    Ok(Frame::Complete(sequence, payload))
}
impl Store {
    pub(super) fn open(
        path: &Path,
        owner: &str,
        initial: State,
    ) -> std::result::Result<Self, BoxError> {
        if !path.exists() {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            File::open(parent)?.sync_all()?;
        }
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(
                "Security directory must be private, owned by the service UID, and not a symlink"
                    .into(),
            );
        }
        let directory: File = openat(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        let mut lock = file(&directory, "lock", true)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        let initialized = lock.metadata()?.len() != 0;
        if initialized {
            let mut marker = [0; 4];
            lock.read_exact(&mut marker)?;
            if &marker != MAGIC || lock.metadata()?.len() != 4 {
                return Err("Invalid security initialization marker".into());
            }
        }
        let mut snapshot = file(&directory, "snapshot", !initialized)?;
        let journal = file(&directory, "journal", !initialized)?;
        if journal.metadata()?.len() > 16 * 1024 * 1024 {
            return Err("Security journal exceeds recovery size limit".into());
        }
        let (state, sequence) = match read_frame(&mut snapshot, MAX_SNAPSHOT)? {
            Frame::End => {
                // A zero-byte snapshot is only valid during first initialization.
                if initialized || journal.metadata()?.len() != 0 {
                    return Err("Missing security baseline".into());
                }
                super::budget::Budget::check_state(&initial, owner)?;
                (initial, 0)
            }
            Frame::Torn => return Err("Incomplete security snapshot".into()),
            Frame::Complete(sequence, payload) => {
                if !matches!(read_frame(&mut snapshot, MAX_SNAPSHOT)?, Frame::End) {
                    return Err("Trailing security snapshot data".into());
                }
                let snapshot: Snapshot<State> = serde_json::from_slice(&payload)?;
                if snapshot.version != 1 || snapshot.owner != owner || snapshot.sequence != sequence
                {
                    return Err("Security snapshot owner/version/sequence mismatch".into());
                }
                (snapshot.state, sequence)
            }
        };
        let mut store = Self {
            directory,
            journal,
            lock,
            initialized,
            owner: owner.into(),
            state,
            sequence,
            snapshot_sequence: sequence,
            length: 0,
        };
        store.replay()?;
        Ok(store)
    }
    fn replay(&mut self) -> std::result::Result<(), BoxError> {
        let mut offset = 0;
        let mut previous = None;
        loop {
            match read_frame(&mut self.journal, MAX_RECORD)? {
                Frame::End => break,
                Frame::Torn => {
                    self.journal.set_len(offset)?;
                    self.journal.sync_all()?;
                    break;
                }
                Frame::Complete(sequence, payload) => {
                    let record: Record<Change> = serde_json::from_slice(&payload)?;
                    if record.version != 1
                        || record.owner != self.owner
                        || sequence == 0
                        || previous.is_some_and(|old: u64| old.checked_add(1) != Some(sequence))
                    {
                        return Err("Invalid security journal version or sequence gap".into());
                    }
                    previous = Some(sequence);
                    if sequence > self.snapshot_sequence {
                        if self.sequence.checked_add(1) != Some(sequence) {
                            return Err("Security journal does not continue the snapshot".into());
                        }
                        self.state.apply(&record.change);
                        self.sequence = sequence;
                    }
                    offset = self.journal.stream_position()?;
                }
            }
        }
        self.length = offset;
        Ok(())
    }
    pub(super) fn checkpoint(&mut self) -> io::Result<()> {
        let snapshot = Snapshot {
            version: 1,
            owner: self.owner.clone(),
            sequence: self.sequence,
            state: &self.state,
        };
        let bytes = encode(self.sequence, &snapshot, MAX_SNAPSHOT)?;
        let name = "snapshot.tmp";
        let temporary = file(&self.directory, name, true)?;
        let result = (|| {
            let mut temporary = temporary;
            temporary.set_len(0)?;
            temporary.write_all(&bytes)?;
            temporary.sync_all()?;
            rustix::fs::renameat(&self.directory, name, &self.directory, "snapshot")?;
            self.directory.sync_all()?;
            // Snapshot rename is durable before old journal bytes may be removed.
            self.journal.set_len(0)?;
            self.journal.seek(SeekFrom::Start(0))?;
            self.journal.sync_all()?;
            self.length = 0;
            self.snapshot_sequence = self.sequence;
            if !self.initialized {
                self.lock.seek(SeekFrom::Start(0))?;
                self.lock.write_all(MAGIC)?;
                self.lock.sync_all()?;
                self.directory.sync_all()?;
                self.initialized = true;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.directory, name, rustix::fs::AtFlags::empty());
        }
        result
    }
    fn append(&mut self, entries: &[Entry], bytes: &[u8]) -> io::Result<()> {
        // A retry overwrites any partial previous attempt from this same batch.
        self.journal.set_len(self.length)?;
        self.journal.seek(SeekFrom::Start(self.length))?;
        self.journal.write_all(bytes)?;
        self.journal.sync_all()?;
        self.length += bytes.len() as u64;
        for entry in entries {
            self.state.apply(&entry.change);
            self.sequence = entry.sequence;
        }
        Ok(())
    }
    pub(super) fn run(
        mut self,
        receiver: mpsc::Receiver<Entry>,
        shared: Arc<Shared>,
        compact_after: u64,
    ) {
        let mut pending: Vec<Entry> = Vec::new();
        let mut encoded = Vec::new();
        loop {
            let result = (|| -> io::Result<()> {
                if self.sequence - self.snapshot_sequence >= compact_after
                    || self.length >= COMPACT_BYTES
                {
                    self.checkpoint()?;
                    let mut status = shared.status();
                    status.snapshot_sequence = self.snapshot_sequence;
                    status.last_error = None;
                }
                if pending.is_empty() {
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(entry) => pending.push(entry),
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                        Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                    }
                    for _ in 1..32 {
                        match receiver.try_recv() {
                            Ok(entry) => pending.push(entry),
                            Err(_) => break,
                        }
                    }
                }
                encoded.clear();
                for entry in &pending {
                    let record = Record {
                        version: 1,
                        owner: self.owner.as_str(),
                        change: &entry.change,
                    };
                    encoded.extend(encode(entry.sequence, &record, MAX_RECORD)?);
                }
                self.append(&pending, &encoded)?;
                {
                    let mut status = shared.status();
                    status.durable_sequence = self.sequence;
                    status.last_error = None;
                    shared.progress.notify_all();
                }
                pending.clear();
                Ok(())
            })();
            if let Err(error) = result {
                let message = error.to_string();
                let changed = {
                    let mut status = shared.status();
                    let changed = status.last_error.as_ref() != Some(&message);
                    status.last_error = Some(message.clone());
                    shared.progress.notify_all();
                    changed
                };
                if changed {
                    eprintln!("mynofy: security persistence pending; retrying: {message}");
                }
                if shared.stopping.load(Ordering::Acquire) {
                    eprintln!("mynofy: security writer stopped with unsaved state: {error}");
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            if pending.is_empty() && shared.stopping.load(Ordering::Acquire) {
                // Drain already accepted changes before a normal shutdown.
                match receiver.try_recv() {
                    Ok(entry) => pending.push(entry),
                    Err(_) => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mnj-{:016x}",
                u64::from_le_bytes(crypto::random().unwrap())
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store::open(&self.0, "owner", State::default()).unwrap()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn change(disabled: bool) -> Change {
        Change::Client {
            value: Client {
                client_id: "test-client".into(),
                display_name: None,
                disabled,
                keys: vec![],
            },
        }
    }
    fn frame(sequence: u64, disabled: bool) -> Vec<u8> {
        encode(
            sequence,
            &Record {
                version: 1,
                owner: "owner",
                change: change(disabled),
            },
            MAX_RECORD,
        )
        .unwrap()
    }
    fn initialized(directory: &Directory) {
        let mut store = directory.store();
        store.checkpoint().unwrap();
    }
    fn write_journal(directory: &Directory, bytes: &[u8]) {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(directory.0.join("journal"))
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    #[test]
    fn replays_valid_prefix_and_truncates_only_an_incomplete_final_frame() {
        let first = frame(1, false);
        let second = frame(2, true);
        for length in [0, 1, 15, 47, 79, 80, second.len() - 1] {
            let directory = Directory::new();
            initialized(&directory);
            let mut bytes = first.clone();
            bytes.extend_from_slice(&second[..length]);
            write_journal(&directory, &bytes);
            let store = directory.store();
            assert_eq!(store.sequence, 1);
            assert!(!store.state.clients["test-client"].disabled);
            assert_eq!(
                fs::metadata(directory.0.join("journal")).unwrap().len(),
                first.len() as u64
            );
        }
    }
    #[test]
    fn rejects_corruption_sequence_gaps_and_unknown_versions_even_at_the_tail() {
        let directory = Directory::new();
        initialized(&directory);
        let first = frame(1, false);
        let mut corrupt_header = frame(2, true);
        corrupt_header[4] ^= 1;
        let mut corrupt_payload = frame(2, true);
        corrupt_payload[HEADER] ^= 1;
        let wrong_owner = encode(
            2,
            &Record {
                version: 1,
                owner: "other",
                change: change(true),
            },
            MAX_RECORD,
        )
        .unwrap();
        let unknown = encode(
            2,
            &Record {
                version: 2,
                owner: "owner",
                change: change(true),
            },
            MAX_RECORD,
        )
        .unwrap();
        for suffix in [
            corrupt_header,
            corrupt_payload,
            frame(3, true),
            unknown,
            wrong_owner,
        ] {
            let mut bytes = first.clone();
            bytes.extend(suffix);
            write_journal(&directory, &bytes);
            assert!(Store::open(&directory.0, "owner", State::default()).is_err());
        }
        let mut bytes = first.clone();
        bytes[HEADER] ^= 1;
        bytes.extend(frame(2, true));
        write_journal(&directory, &bytes);
        assert!(Store::open(&directory.0, "owner", State::default()).is_err());
    }
    #[test]
    fn snapshot_survives_old_journal_and_replay_compacts_to_one_state() {
        let directory = Directory::new();
        initialized(&directory);
        let first = frame(1, false);
        write_journal(&directory, &first);
        {
            let mut store = directory.store();
            store.checkpoint().unwrap();
        }
        // Crash after durable snapshot rename, before old journal truncation.
        let mut bytes = first;
        bytes.extend(frame(2, true));
        write_journal(&directory, &bytes);
        {
            let mut store = directory.store();
            assert_eq!(store.sequence, 2);
            assert!(store.state.clients["test-client"].disabled);
            store.checkpoint().unwrap();
        }
        assert_eq!(fs::metadata(directory.0.join("journal")).unwrap().len(), 0);
        assert_eq!(directory.store().sequence, 2);
    }
    #[test]
    fn private_storage_is_locked_and_rejects_missing_baseline_symlinks_and_wrong_owner() {
        let directory = Directory::new();
        initialized(&directory);
        let store = directory.store();
        assert!(Store::open(&directory.0, "owner", State::default()).is_err());
        drop(store);
        assert!(Store::open(&directory.0, "different-owner", State::default()).is_err());
        fs::remove_file(directory.0.join("snapshot")).unwrap();
        assert!(Store::open(&directory.0, "owner", State::default()).is_err());
        assert!(!directory.0.join("snapshot").exists());
        let directory = Directory::new();
        initialized(&directory);
        let target = directory.0.join("target");
        fs::write(&target, b"keep").unwrap();
        fs::remove_file(directory.0.join("journal")).unwrap();
        symlink(&target, directory.0.join("journal")).unwrap();
        assert!(Store::open(&directory.0, "owner", State::default()).is_err());
        assert_eq!(fs::read(target).unwrap(), b"keep");
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::open(&directory.0, "owner", State::default()).is_err());
    }
    #[test]
    fn failed_write_does_not_advance_state_and_retry_replaces_partial_bytes() {
        let directory = Directory::new();
        initialized(&directory);
        let mut store = directory.store();
        store.journal = File::open(directory.0.join("journal")).unwrap();
        let shared = Arc::new(Shared {
            progress: std::sync::Condvar::new(),
            status: Mutex::new(Status::default()),
            pending: AtomicUsize::new(1),
            stopping: AtomicBool::new(false),
            alive: AtomicBool::new(true),
        });
        let entries = [Entry {
            sequence: 1,
            change: change(true),
            _reservation: Reservation(shared),
        }];
        let bytes = frame(1, true);
        assert!(store.append(&entries, &bytes).is_err());
        assert_eq!(store.sequence, 0);
        assert!(store.state.clients.is_empty());
        write_journal(&directory, &bytes[..50]);
        store.journal = file(&store.directory, "journal", false).unwrap();
        store.append(&entries, &bytes).unwrap();
        assert_eq!(store.sequence, 1);
        assert!(store.state.clients["test-client"].disabled);
        assert_eq!(fs::read(directory.0.join("journal")).unwrap(), bytes);
        drop(store);
        assert_eq!(directory.store().sequence, 1);
    }
    #[test]
    fn bounded_serialization_and_snapshot_checksum_are_enforced() {
        assert!(encode(1, &vec![0u8; 100], 10).is_err());
        let directory = Directory::new();
        initialized(&directory);
        let path = directory.0.join("snapshot");
        let mut bytes = fs::read(&path).unwrap();
        bytes[HEADER] ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(Store::open(&directory.0, "owner", State::default()).is_err());
    }
}
