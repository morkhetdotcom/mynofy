use super::*;
use rustix::fs::{AtFlags, CWD, FlockOperation, Mode, OFlags, flock, openat, renameat, unlinkat};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::Path,
};
pub(super) struct Store {
    directory: File,
    lock: File,
    uncertain: AtomicBool,
    cleanup_error: Mutex<Option<String>>,
}
// Release the owner's flock immediately, even if a concurrent process spawn briefly
// inherited a duplicate before exec closes its CLOEXEC descriptors.
impl Drop for Store {
    fn drop(&mut self) {
        let _ = flock(&self.lock, FlockOperation::Unlock);
    }
}
fn open(directory: &File, name: &str, flags: OFlags) -> std::result::Result<File, BoxError> {
    let file: File = openat(
        directory,
        name,
        flags | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?
    .into();
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err("Content files must be private regular files owned by the service UID".into());
    }
    Ok(file)
}
impl Store {
    pub fn open(path: &Path) -> std::result::Result<Self, BoxError> {
        Self::open_mode(path, false)
    }

    pub(super) fn open_mode(path: &Path, preview: bool) -> std::result::Result<Self, BoxError> {
        Self::open_inner(path, preview, None)
    }

    pub(super) fn open_bound(path: &Path, id: &str) -> std::result::Result<Self, BoxError> {
        Self::open_inner(path, false, Some(id))
    }

    fn open_inner(
        path: &Path,
        preview: bool,
        expected_id: Option<&str>,
    ) -> std::result::Result<Self, BoxError> {
        if !preview && expected_id.is_none() && !path.exists() {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            File::open(
                path.parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new(".")),
            )?
            .sync_all()?;
        }
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
        {
            return Err(
                "Content directory must be private, owned by the service UID and not a symlink"
                    .into(),
            );
        }
        let directory: File = openat(
            CWD,
            path,
            OFlags::DIRECTORY | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        let lock_flags = if preview {
            OFlags::RDONLY
        } else if expected_id.is_some() {
            OFlags::RDWR
        } else {
            OFlags::CREATE | OFlags::RDWR
        };
        let lock = open(&directory, "lock", lock_flags)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        let store = Self {
            directory,
            lock,
            uncertain: AtomicBool::new(false),
            cleanup_error: Mutex::new(None),
        };
        if let Some(id) = expected_id {
            store.verify_id(id)?;
        }
        if preview {
            return Ok(store);
        }
        // A previous process may have stopped after rename but before directory sync.
        // Confirm the visible namespace before any old slot can be overwritten or removed.
        #[cfg(test)]
        super::faults::hit("open.confirm_dir_sync")?;
        store.directory.sync_all()?;
        if store.has_state()? {
            store.mark(b"MYNCOM01")?;
        } else if store.lock.metadata()?.len() == 0 && !store.has_topic_files()? {
            store.mark(b"MYNFRESH")?;
        }
        Ok(store)
    }

    pub(super) fn verify_id(&self, id: &str) -> std::result::Result<(), BoxError> {
        let mut file = open(&self.directory, "store-id", OFlags::RDONLY)
            .map_err(|_| "Content store identity missing; mount the expected volume or explicitly initialize it")?;
        let mut stored = Vec::new();
        Read::by_ref(&mut file).take(33).read_to_end(&mut stored)?;
        if stored != id.as_bytes() {
            return Err("Content store identity does not match the configuration".into());
        }
        Ok(())
    }

    pub(super) fn initialize(path: &Path, id: &str) -> std::result::Result<(), BoxError> {
        let store = Self::open(path)?;
        if !store.has_state()? {
            store.require_fresh()?;
        }
        match open(&store.directory, "store-id", OFlags::RDONLY) {
            Ok(_) => store.verify_id(id)?,
            Err(error)
                if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT) =>
            {
                store.write_id(id)?;
            }
            Err(error) => return Err(error),
        }
        if !store.has_state()? {
            store.mark(b"MYNFRI02")?;
        }
        Ok(())
    }

    pub(super) fn write_id(&self, id: &str) -> std::result::Result<(), BoxError> {
        self.replace("store-id.tmp", "store-id", id.as_bytes(), true)
    }

    pub(super) fn begin_backup(&self) -> std::result::Result<(), BoxError> {
        self.mark(b"MYNBACK1")
    }

    fn mark(&self, marker: &[u8; 8]) -> std::result::Result<(), BoxError> {
        let mut lock = &self.lock;
        lock.seek(SeekFrom::Start(0))?;
        lock.write_all(marker)?;
        lock.set_len(8)?;
        lock.sync_all()?;
        Ok(())
    }
    fn has_topic_files(&self) -> std::result::Result<bool, BoxError> {
        for entry in rustix::fs::Dir::read_from(&self.directory)? {
            let entry = entry?;
            if entry.file_name().to_bytes().starts_with(b"topic-") {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn require_fresh(&self) -> std::result::Result<(), BoxError> {
        self.check_fresh(false)
    }

    pub(super) fn check_fresh(&self, bound: bool) -> std::result::Result<(), BoxError> {
        let mut marker = Vec::new();
        let mut lock = &self.lock;
        lock.seek(SeekFrom::Start(0))?;
        lock.take(9).read_to_end(&mut marker)?;
        if marker == b"MYNFRI02" || (!bound && marker == b"MYNFRESH") {
            return Ok(());
        }
        Err("Content manifest missing from a store without an explicit fresh marker; files preserved".into())
    }

    pub(super) fn require_committed(&self) -> std::result::Result<(), BoxError> {
        let mut marker = [0; 8];
        let mut lock = &self.lock;
        lock.seek(SeekFrom::Start(0))?;
        if lock.metadata()?.len() != 8
            || lock.read_exact(&mut marker).is_err()
            || &marker != b"MYNCOM01"
            || !self.has_state()?
        {
            return Err("Restore source has no committed content manifest".into());
        }
        Ok(())
    }

    pub fn has_state(&self) -> std::result::Result<bool, BoxError> {
        match open(&self.directory, "state.bin", OFlags::RDONLY) {
            Ok(_) => Ok(true),
            Err(error)
                if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Remove only internally generated, uncommitted topic slots after startup validation.
    pub fn cleanup_topics(
        &self,
        keep: &std::collections::HashSet<String>,
    ) -> std::result::Result<(), BoxError> {
        let directory = rustix::fs::Dir::read_from(&self.directory)?;
        let mut removed = false;
        for entry in directory {
            let entry = entry?;
            let Ok(name) = entry.file_name().to_str() else {
                continue;
            };
            let bytes = name.as_bytes();
            if bytes.len() == 76
                && bytes.starts_with(b"topic-")
                && bytes[6..70]
                    .iter()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
                && bytes[70] == b'-'
                && matches!(bytes[71], b'0' | b'1')
                && &bytes[72..] == b".bin"
                && !keep.contains(name)
            {
                self.remove_named_checked(name)?;
                removed = true;
            }
        }
        if removed {
            self.directory.sync_all()?;
        }
        Ok(())
    }
    pub fn read(&self, maximum: u64) -> std::result::Result<Option<Zeroizing<Vec<u8>>>, BoxError> {
        self.read_named("state.bin", maximum)
    }
    pub fn read_named(
        &self,
        name: &str,
        maximum: u64,
    ) -> std::result::Result<Option<Zeroizing<Vec<u8>>>, BoxError> {
        let file = match open(&self.directory, name, OFlags::RDONLY) {
            Ok(file) => file,
            Err(error) => {
                if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT) {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        if file.metadata()?.len() > maximum {
            return Err("Content snapshot exceeds configured size limit".into());
        }
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(maximum + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > maximum {
            return Err("Content snapshot exceeds configured size limit".into());
        }
        Ok(Some(bytes))
    }
    pub fn save(&self, bytes: &[u8]) -> std::result::Result<(), BoxError> {
        self.replace("state.tmp", "state.bin", bytes, true)
    }
    pub fn uncertain(&self) -> bool {
        self.uncertain.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub fn save_named(&self, name: &str, bytes: &[u8]) -> std::result::Result<(), BoxError> {
        self.replace("topic.tmp", name, bytes, true)
    }
    // Data must reach disk before the final manifest can refer to this slot.
    // The writer synchronizes the directory once after staging the entire batch.
    pub fn stage_topic(&self, name: &str, bytes: &[u8]) -> std::result::Result<(), BoxError> {
        self.replace("topic.tmp", name, bytes, false)
    }
    pub fn sync_staged_topics(&self) -> std::result::Result<(), BoxError> {
        #[cfg(test)]
        super::faults::hit("topics.dir_sync")?;
        self.directory.sync_all()?;
        #[cfg(test)]
        super::faults::hit("topics.dir_synced")?;
        Ok(())
    }
    pub fn remove_named(&self, name: &str) {
        if let Err(error) = self.remove_named_checked(name) {
            let message = format!("Could not remove old content file {name}: {error}");
            eprintln!("mynofy: {message}");
            *self.cleanup_error.lock().unwrap() = Some(message);
        }
    }
    pub fn take_cleanup_error(&self) -> Option<String> {
        self.cleanup_error.lock().unwrap().take()
    }
    fn remove_named_checked(&self, name: &str) -> std::io::Result<()> {
        #[cfg(test)]
        super::faults::hit("cleanup.before_unlink")?;
        match unlinkat(&self.directory, name, AtFlags::empty()) {
            Ok(()) => (),
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        #[cfg(test)]
        super::faults::hit("cleanup.after_unlink")?;
        Ok(())
    }
    pub fn cleanup_temporary(&self) -> std::result::Result<(), BoxError> {
        for name in ["topic.tmp", "state.tmp", "references.tmp"] {
            self.remove_named_checked(name)?;
        }
        #[cfg(test)]
        super::faults::hit("cleanup.dir_sync")?;
        self.directory.sync_all()?;
        Ok(())
    }
    fn replace(
        &self,
        name: &str,
        target: &str,
        bytes: &[u8],
        sync_directory: bool,
    ) -> std::result::Result<(), BoxError> {
        if self.uncertain() {
            return Err(
                "Content manifest durability is uncertain; restart before saving again".into(),
            );
        }
        #[cfg(test)]
        super::faults::phase(target, "open")?;
        let mut file = open(&self.directory, name, OFlags::RDWR | OFlags::CREATE)?;
        let result = (|| -> std::result::Result<(), BoxError> {
            #[cfg(test)]
            super::faults::phase(target, "truncate")?;
            file.set_len(0)?;
            #[cfg(test)]
            super::faults::phase(target, "truncated")?;
            #[cfg(test)]
            {
                let half = bytes.len().div_ceil(2);
                file.write_all(&bytes[..half])?;
                super::faults::phase(target, "partial_written")?;
                file.write_all(&bytes[half..])?;
            }
            #[cfg(not(test))]
            file.write_all(bytes)?;
            #[cfg(test)]
            super::faults::phase(target, "written")?;
            #[cfg(test)]
            super::faults::phase(target, "file_sync")?;
            file.sync_all()?;
            #[cfg(test)]
            super::faults::phase(target, "file_synced")?;
            #[cfg(test)]
            super::faults::phase(target, "rename")?;
            renameat(&self.directory, name, &self.directory, target)?;
            if target == "state.bin" {
                self.uncertain.store(true, Ordering::Release);
            }
            #[cfg(test)]
            super::faults::phase(target, "renamed")?;
            if sync_directory {
                #[cfg(test)]
                super::faults::phase(target, "dir_sync")?;
                self.directory.sync_all()?;
                #[cfg(test)]
                super::faults::phase(target, "dir_synced")?;
            }
            if target == "state.bin" {
                self.mark(b"MYNCOM01")?;
                self.uncertain.store(false, Ordering::Release);
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = unlinkat(&self.directory, name, AtFlags::empty());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn directory() -> Directory {
        let path = std::env::temp_dir().join(format!(
            "mnd-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Directory(path)
    }
    #[test]
    fn content_files_are_private_locked_and_symlink_replacements_are_not_followed() {
        let dir = directory();
        let store = Store::open(&dir.0).unwrap();
        assert!(store.read(100).unwrap().is_none());
        assert!(Store::open(&dir.0).is_err());
        store.save(b"old checkpoint").unwrap();
        assert_eq!(
            fs::metadata(dir.0.join("state.bin")).unwrap().mode() & 0o777,
            0o600
        );
        let target = dir.0.join("keep");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, dir.0.join("state.tmp")).unwrap();
        assert!(store.save(b"new checkpoint").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert_eq!(
            fs::read(dir.0.join("state.bin")).unwrap(),
            b"old checkpoint"
        );
        fs::remove_file(dir.0.join("state.tmp")).unwrap();
        store.save(b"new checkpoint").unwrap();
        assert_eq!(
            store.read(100).unwrap().unwrap().as_slice(),
            b"new checkpoint"
        );
        assert!(store.read(1).is_err());
        drop(store);
        fs::remove_file(dir.0.join("state.bin")).unwrap();
        symlink(target, dir.0.join("state.bin")).unwrap();
        assert!(
            Store::open(&dir.0)
                .and_then(|store| store.read(100))
                .is_err()
        );
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::open(&dir.0).is_err());
    }
}

#[cfg(test)]
mod topic_slot_tests {
    use super::*;
    #[test]
    fn startup_cleanup_only_unlinks_uncommitted_internal_slots_and_uncertain_commit_blocks_writes()
    {
        let path = std::env::temp_dir().join(format!(
            "mnt-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let store = Store::open(&path).unwrap();
        store.save(b"committed manifest").unwrap();
        let committed = format!("topic-{}-0.bin", "a".repeat(64));
        let orphan = format!("topic-{}-1.bin", "b".repeat(64));
        store.save_named(&committed, b"committed topic").unwrap();
        fs::write(path.join("user-file"), b"keep user file").unwrap();
        std::os::unix::fs::symlink(path.join("user-file"), path.join(&orphan)).unwrap();
        store
            .cleanup_topics(&[committed.clone()].into_iter().collect())
            .unwrap();
        assert!(path.join(&committed).exists());
        assert!(!path.join(&orphan).exists());
        assert_eq!(fs::read(path.join("user-file")).unwrap(), b"keep user file");
        assert_eq!(
            store.read(100).unwrap().unwrap().as_slice(),
            b"committed manifest"
        );
        // Model the documented state after rename succeeded but directory sync failed.
        store.uncertain.store(true, Ordering::Release);
        assert!(store.save(b"new manifest").is_err());
        assert!(
            store
                .save_named(&committed, b"overwrite referenced data")
                .is_err()
        );
        assert_eq!(fs::read(path.join(&committed)).unwrap(), b"committed topic");
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn store_drop_releases_flock_even_while_a_duplicate_descriptor_exists() {
        let path = std::env::temp_dir().join(format!(
            "mynofy-lock-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        let store = Store::open(&path).unwrap();
        let duplicate = store.lock.try_clone().unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        drop(duplicate);
        assert!(Store::open(&path).is_err());
        drop(reopened);
        fs::remove_dir_all(path).unwrap();
    }
}
