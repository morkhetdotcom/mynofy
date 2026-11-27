use super::*;
use mynofy_client::progress::ClientProgress;
use rustix::fs::{CWD, FlockOperation, Mode, OFlags, flock, openat};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
};
pub struct ProgressFile {
    path: std::path::PathBuf,
    _lock: File,
}
impl ProgressFile {
    pub fn open(path: &Path) -> Result<(Self, ClientProgress), BoxError> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let metadata = std::fs::metadata(parent)?;
        if !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
            return Err("Progress directory must not be writable by group/others".into());
        }
        let path = std::fs::canonicalize(parent)?
            .join(path.file_name().ok_or("Missing progress filename")?);
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock: File = openat(
            CWD,
            Path::new(&lock_path),
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?
        .into();
        private(&lock)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        let state = match openat(
            CWD,
            &path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file: File = fd.into();
                private(&file)?;
                let mut bytes = Vec::new();
                file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err("Progress file exceeds size limit".into());
                }
                serde_json::from_value(mynofy_protocol::strict_json(std::str::from_utf8(&bytes)?)?)?
            }
            Err(rustix::io::Errno::NOENT) => ClientProgress::default(),
            Err(error) => return Err(error.into()),
        };
        Ok((Self { path, _lock: lock }, state))
    }
    pub fn save(&self, state: &ClientProgress) -> Result<(), BoxError> {
        self.save_encoded(&Self::encode(state)?)
    }

    pub fn encode(state: &ClientProgress) -> Result<Vec<u8>, BoxError> {
        let bytes = serde_json::to_vec(state)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Progress file exceeds size limit".into());
        }
        Ok(bytes)
    }

    pub fn save_encoded(&self, bytes: &[u8]) -> Result<(), BoxError> {
        let parent = self.path.parent().ok_or("Missing progress parent")?;
        // Never open/truncate a pre-existing staging file, even if it is private.
        let (mut file, mut temporary) = (0..16)
            .find_map(|_| -> Option<Result<(File, Temporary), BoxError>> {
                let random = crypto::random::<16>();
                let random = match random {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e.into())),
                };
                let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
                let path = parent.join(format!(".mynofy-progress-{suffix}.tmp"));
                match openat(
                    CWD,
                    &path,
                    OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::WRONLY
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                ) {
                    Ok(fd) => Some(Ok((File::from(fd), Temporary(Some(path))))),
                    Err(rustix::io::Errno::EXIST) => None,
                    Err(error) => Some(Err(error.into())),
                }
            })
            .ok_or("Could not allocate a unique progress staging file")??;
        private(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(
            temporary
                .0
                .as_ref()
                .ok_or("Missing progress staging path")?,
            &self.path,
        )?;
        temporary.0 = None;
        File::open(self.path.parent().ok_or("Missing progress parent")?)?.sync_all()?;
        Ok(())
    }
}
fn private(file: &File) -> Result<(), BoxError> {
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.mode() & 0o077 != 0 || m.uid() != current_uid() {
        return Err("Progress files must be private regular files".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}
#[cfg(not(target_os = "linux"))]
fn current_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

// Only a path exclusively created by this save may be removed on failure.
struct Temporary(Option<std::path::PathBuf>);
impl Drop for Temporary {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    #[test]
    fn saves_preserve_neighbors_and_remove_only_their_own_failed_staging_file() {
        let suffix = u64::from_le_bytes(crypto::random().unwrap());
        let directory = std::env::temp_dir().join(format!("mnp-{suffix:016x}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let path = directory.join("progress.json");
        let neighbor = directory.join("progress.json.tmp");
        let marker = b"unrelated private data";
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&neighbor)
            .unwrap();
        file.write_all(marker).unwrap();
        let (progress, state) = ProgressFile::open(&path).unwrap();
        progress.save(&state).unwrap();
        progress.save(&state).unwrap();
        assert!(
            ProgressFile::open(&path).is_err(),
            "exclusive lock must remain held"
        );
        std::fs::remove_file(&path).unwrap();
        // Force rename failure after staging allocation and writing.
        std::fs::create_dir(&path).unwrap();
        assert!(progress.save(&state).is_err());
        assert_eq!(std::fs::read(&neighbor).unwrap(), marker);
        assert!(path.is_dir());
        assert!(!std::fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".mynofy-progress-")
        }));
    }
}
