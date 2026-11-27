//! Filesystem-protected, same-user local WebSocket transport. No TLS credentials needed.
use super::*;
use std::{
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};
use tokio::net::UnixListener;

pub struct BoundUnixSocket {
    pub(crate) listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    pub(crate) uid: u32,
}
impl Drop for BoundUnixSocket {
    fn drop(&mut self) {
        // Never remove a replacement file/socket or a symlink installed at the old name.
        if let Ok(meta) = std::fs::symlink_metadata(&self.path)
            && meta.file_type().is_socket()
            && meta.dev() == self.device
            && meta.ino() == self.inode
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
fn socket_path(path: &Path, create_parent: bool) -> std::result::Result<PathBuf, BoxError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if create_parent {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let parent = std::fs::canonicalize(parent)?;
    let metadata = std::fs::metadata(&parent)?;
    if !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
        return Err("Unix socket parent must not be writable by group or others".into());
    }
    let name = path.file_name().ok_or("Missing Unix socket filename")?;
    Ok(parent.join(name))
}
pub fn bind_unix(path: &Path) -> std::result::Result<BoundUnixSocket, BoxError> {
    let path = socket_path(path, true)?;
    // Existing entries, including stale sockets, are deliberately not removed.
    let listener = UnixListener::bind(&path)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    let bound = BoundUnixSocket {
        listener,
        path,
        device: metadata.dev(),
        inode: metadata.ino(),
        uid: metadata.uid(),
    };
    std::fs::set_permissions(&bound.path, std::fs::Permissions::from_mode(0o600))?;
    Ok(bound)
}
pub async fn run_unix(bound: BoundUnixSocket, config: Config) -> std::result::Result<(), BoxError> {
    run_unix_prepared(bound, prepare(config, None)?).await
}
pub async fn run_unix_prepared(
    bound: BoundUnixSocket,
    prepared: PreparedService,
) -> std::result::Result<(), BoxError> {
    run_unix_prepared_until(bound, prepared, std::future::pending()).await
}

/// Stop accepting work when the shutdown future completes and report the durable drain.
pub async fn run_unix_prepared_until(
    bound: BoundUnixSocket,
    prepared: PreparedService,
    shutdown: impl std::future::Future<Output = std::io::Result<()>>,
) -> std::result::Result<(), BoxError> {
    tokio::pin!(shutdown);
    let mut prepared = prepared;
    if prepared.state.config.service.unix_socket.is_none() || prepared.tls.is_some() {
        return Err("Unix listener requires unix_socket without TLS".into());
    }
    let mut admin = prepared.take_admin()?;
    let state = prepared.state;
    let mut tasks = JoinSet::new();
    let mut maintenance = Box::pin(tokio::time::sleep(Duration::from_secs(1)));
    let result = async {
        loop {
        tokio::select! {
            stopped=&mut shutdown=>{return stopped.map_err(Into::into);},
            result=&mut admin=>{return result;},
            accepted=bound.listener.accept()=>{
                let(stream,_)=accepted?;
                let Ok(credentials)=stream.peer_cred() else{continue};
                let uid=credentials.uid();
                if uid!=bound.uid{continue;}
                let Some(admission)=state.admit(SourceId::Uid(uid))? else{continue};
                let state=state.clone();
                tasks.spawn(async move{
                    let _admission=admission;
                    let _=websocket_connection(stream,state.engine.clone(),&state.config,Some(&state.network),None,None,None).await;
                });
            },
            _=&mut maintenance=>{
                let progress = locked(&state.engine)?.maintenance(now_ms());
                let delay = if progress.more { Duration::from_millis(10) } else { Duration::from_secs(1) };
                maintenance.as_mut().reset(Instant::now() + delay);
            },
            _=tasks.join_next(),if !tasks.is_empty()=>{},
        }
        }
    }.await;
    drop(admin);
    drop(bound);
    tasks.shutdown().await;
    state.network.shutdown().await;
    let drained = finish_shutdown(state.engine.clone()).await;
    drained.and(result)
}
