//! Resumable file downloader with progress events and integrity checks.
//!
//! Every asset lands next to its final path as `<name>.part` while in flight, so an
//! interrupted download resumes with an HTTP Range request on the next attempt.

use super::registry::Asset;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::io::AsyncWriteExt;

/// Payload of the `engine-download-progress` event. One event roughly every 250 ms
/// per asset, plus one final event with `done: true`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub asset_id: String,
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
    pub done: bool,
    pub error: Option<String>,
}

/// True when the asset already exists at `dest` with the expected size.
/// (Hash verification happens once, right after download; re-hashing gigabytes on
/// every launch is not worth it.)
pub fn is_complete(asset: &Asset, dest: &Path) -> bool {
    std::fs::metadata(dest).map(|m| m.len() == asset.size).unwrap_or(false)
}

/// Download `asset` to `dest`, resuming a partial file if present. Verifies size and,
/// when the registry has one, the sha256. Emits progress on `app`.
pub async fn download_asset(app: &tauri::AppHandle, asset: &Asset, dest: &Path) -> Result<(), String> {
    if is_complete(asset, dest) {
        emit(app, asset, asset.size, 0, true, None);
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }

    let part: PathBuf = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    ));
    let mut downloaded = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if downloaded >= asset.size {
        // A stale or oversized partial cannot be trusted; start over.
        let _ = std::fs::remove_file(&part);
        downloaded = 0;
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // No overall timeout: multi-gigabyte files on slow links legitimately take a long time.
        // Stalls are caught by the per-chunk read timeout below.
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(&asset.url).header("User-Agent", "local-mca-underwriter");
    if downloaded > 0 {
        req = req.header("Range", format!("bytes={downloaded}-"));
    }
    let resp = req.send().await.map_err(|e| format!("Download failed to start: {e}"))?;
    let status = resp.status();
    let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !status.is_success() {
        let msg = format!("Server returned {status} for {}", asset.url);
        emit(app, asset, downloaded, 0, true, Some(msg.clone()));
        return Err(msg);
    }
    if downloaded > 0 && !resuming {
        // Server ignored the Range header; it is sending the whole file.
        downloaded = 0;
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resuming)
        .truncate(!resuming)
        .open(&part)
        .await
        .map_err(|e| format!("Cannot open {}: {e}", part.display()))?;

    let mut stream = resp.bytes_stream();
    let started = Instant::now();
    let start_offset = downloaded;
    let mut last_emit = Instant::now();

    loop {
        let next = tokio::time::timeout(Duration::from_secs(60), stream.next()).await;
        let chunk = match next {
            Err(_) => {
                let msg = "Download stalled for 60 seconds".to_string();
                emit(app, asset, downloaded, 0, true, Some(msg.clone()));
                return Err(msg);
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                let msg = format!("Download interrupted: {e}");
                emit(app, asset, downloaded, 0, true, Some(msg.clone()));
                return Err(msg);
            }
            Ok(Some(Ok(bytes))) => bytes,
        };
        file.write_all(&chunk).await.map_err(|e| format!("Write failed: {e}"))?;
        downloaded += chunk.len() as u64;

        if last_emit.elapsed() >= Duration::from_millis(250) {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let rate = ((downloaded - start_offset) as f64 / secs) as u64;
            emit(app, asset, downloaded, rate, false, None);
            last_emit = Instant::now();
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    if downloaded != asset.size {
        let msg = format!(
            "Size mismatch for {}: got {downloaded} bytes, expected {}",
            asset.file_name, asset.size
        );
        let _ = std::fs::remove_file(&part);
        emit(app, asset, downloaded, 0, true, Some(msg.clone()));
        return Err(msg);
    }

    if let Some(expected) = asset.sha256 {
        let actual = sha256_file(&part).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            let msg = format!("Checksum mismatch for {}; the download was corrupted", asset.file_name);
            let _ = std::fs::remove_file(&part);
            emit(app, asset, downloaded, 0, true, Some(msg.clone()));
            return Err(msg);
        }
    }

    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| format!("Cannot move {} into place: {e}", part.display()))?;
    emit(app, asset, downloaded, 0, true, None);
    Ok(())
}

/// Streaming sha256 of a file on a blocking thread (files are up to several GB).
async fn sha256_file(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
        Ok(format!("{:x}", hasher.finalize()))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn emit(app: &tauri::AppHandle, asset: &Asset, downloaded: u64, rate: u64, done: bool, error: Option<String>) {
    let _ = app.emit(
        "engine-download-progress",
        DownloadProgress {
            asset_id: asset.id.to_string(),
            downloaded,
            total: asset.size,
            bytes_per_sec: rate,
            done,
            error,
        },
    );
}
