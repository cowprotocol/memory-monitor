use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{error, info};

/// Connect to the jemalloc profiling Unix socket, send `dump\n`, and write the
/// response to `dump_file`. Returns the path on success.
pub async fn create_heap_dump(binary_name: &str, dump_file: &Path) -> Result<(), String> {
    let sock_path = format!("/tmp/heap_dump_{}.sock", binary_name);

    // Check socket exists
    let metadata = tokio::fs::metadata(&sock_path).await.map_err(|e| {
        format!(
            "ERROR: Socket {} does not exist or is not accessible: {}",
            sock_path, e
        )
    })?;

    if !metadata.file_type().is_socket() {
        return Err(format!("ERROR: {} exists but is not a socket", sock_path));
    }

    info!(sock_path, "Connecting to socket to request heap dump...");

    let mut stream = UnixStream::connect(&sock_path)
        .await
        .map_err(|e| format!("ERROR: Failed to connect to socket {}: {}", sock_path, e))?;

    stream
        .write_all(b"dump\n")
        .await
        .map_err(|e| format!("ERROR: Failed to send dump command: {}", e))?;

    info!("Connected, sent dump command. Reading response...");

    let mut data = Vec::new();
    stream
        .read_to_end(&mut data)
        .await
        .map_err(|e| format!("ERROR: Failed to read dump response: {}", e))?;

    info!(bytes = data.len(), "Received dump data");

    if data.is_empty() {
        return Err("ERROR: Received empty dump response".to_string());
    }

    tokio::fs::write(dump_file, &data)
        .await
        .map_err(|e| format!("ERROR: Failed to write dump file {:?}: {}", dump_file, e))?;

    info!(?dump_file, bytes = data.len(), "Heap dump created");

    Ok(())
}

/// Remove a dump file after upload.
pub async fn cleanup_dump_file(path: &Path) {
    if let Err(err) = tokio::fs::remove_file(path).await {
        error!(?path, ?err, "Failed to clean up dump file");
    } else {
        info!(?path, "Cleaned up local dump file");
    }
}
