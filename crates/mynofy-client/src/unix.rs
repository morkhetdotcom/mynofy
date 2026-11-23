//! Protected filesystem endpoint and peer UID validation for the local client.
use super::*;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::PathBuf,
};
use tokio::net::UnixStream;
fn socket_path(path: &Path) -> std::result::Result<PathBuf, BoxError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent)?;
    let metadata = std::fs::metadata(&parent)?;
    if !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
        return Err("Unix socket parent must not be writable by group or others".into());
    }
    let name = path.file_name().ok_or("Missing Unix socket filename")?;
    Ok(parent.join(name))
}
pub async fn connect_unix_client(
    path: &Path,
    pkcs8: &[u8],
    owner: &str,
    expected_client: Option<&str>,
) -> std::result::Result<WebSocketStream<UnixStream>, BoxError> {
    Ok(
        connect_unix_client_info(path, pkcs8, owner, expected_client)
            .await?
            .0,
    )
}
pub async fn connect_unix_client_info(
    path: &Path,
    pkcs8: &[u8],
    owner: &str,
    expected_client: Option<&str>,
) -> std::result::Result<(WebSocketStream<UnixStream>, protocol::pb::AuthWelcome), BoxError> {
    authenticate(open_unix(path).await?, pkcs8, owner, expected_client).await
}
pub async fn connect_unix_via_info(
    path: &Path,
    options: crate::ViaOptions<'_>,
    pkcs8: &[u8],
    expected_client: Option<&str>,
) -> Result<(crate::OwnerSocket<UnixStream>, protocol::pb::AuthWelcome), BoxError> {
    crate::connect_owner_info(open_unix(path).await?, options, pkcs8, expected_client).await
}
async fn open_unix(path: &Path) -> Result<WebSocketStream<UnixStream>, BoxError> {
    let input = path.to_owned();
    let (path, metadata) = tokio::task::spawn_blocking(move || {
        let path = socket_path(&input)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket() || metadata.mode() & 0o077 != 0 {
            return Err(BoxError::from(
                "Expected a Unix socket accessible only to its owner (0600)",
            ));
        }
        Ok::<_, BoxError>((path, metadata))
    })
    .await??;
    let stream = timeout(Duration::from_secs(10), UnixStream::connect(&path)).await??;
    if stream.peer_cred()?.uid() != metadata.uid() {
        return Err("Unix socket server UID does not match the socket owner".into());
    }
    let checked_path = path.clone();
    let after =
        tokio::task::spawn_blocking(move || std::fs::symlink_metadata(checked_path)).await??;
    if after.dev() != metadata.dev() || after.ino() != metadata.ino() {
        return Err("Unix socket changed during connection".into());
    }
    let (ws, _) = timeout(
        Duration::from_secs(10),
        tokio_tungstenite::client_async_with_config(
            "ws://localhost/",
            stream,
            Some(ws_config(4 * 1024 * 1024)),
        ),
    )
    .await??;
    Ok(ws)
}
