//! Download a model: resume what's already there, check it against its SHA-256, and only
//! then move it to the name the app loads. A half-finished or damaged file is never left
//! under that name.
//!
//! The data goes into `<file>.part`. If a download is cut, cancelled or stalls, the `.part`
//! file is kept and the next call asks the server for just the rest (an HTTP `Range`
//! request). A file that finishes but doesn't match its hash is deleted.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::registry::{ModelFile, ModelSpec};

/// How often a blocked read wakes up to check for cancellation and for a stall.
const POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Downloading { downloaded: u64, total: u64 },
    /// All the bytes are here; checking them (takes a few seconds for a large model).
    Verifying,
    Done,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// How long the connection may deliver nothing before the download is called stalled.
    pub stall_timeout: Duration,
    pub connect_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self { stall_timeout: Duration::from_secs(30), connect_timeout: Duration::from_secs(15) }
    }
}

/// Why a download didn't finish. The messages are shown to the person as they are.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DownloadError {
    #[error("Couldn't reach the download server. Check your internet connection and try again.")]
    Offline(String),
    #[error("This model is no longer available at its download address.")]
    NotFound,
    #[error("The download server answered with an error ({0}). Try again in a few minutes.")]
    Http(u16),
    #[error("The download stopped receiving data. Check your connection and try again; it will pick up where it left off.")]
    Stalled,
    #[error("The download was cut short ({got} of {expected} bytes). Try again; it will pick up where it left off.")]
    Truncated { got: u64, expected: u64 },
    #[error("The downloaded file was damaged (its checksum didn't match), so it was discarded. Try again.")]
    HashMismatch { expected: String, actual: String },
    #[error("There isn't enough space on this disk. Free up some space and try again.")]
    DiskFull,
    #[error("Couldn't save the model: {0}")]
    Io(String),
    #[error("Download cancelled.")]
    Cancelled,
}

impl DownloadError {
    pub fn from_io(e: std::io::Error) -> Self {
        // 28 is ENOSPC on macOS, Linux and most Unixes.
        if e.kind() == std::io::ErrorKind::StorageFull || e.raw_os_error() == Some(28) {
            Self::DiskFull
        } else {
            Self::Io(e.to_string())
        }
    }
}

fn from_send(e: reqwest::Error) -> DownloadError {
    if e.is_connect() || e.is_timeout() {
        DownloadError::Offline(e.to_string())
    } else {
        DownloadError::Io(e.to_string())
    }
}

/// The `S` of a `Content-Range: bytes S-E/T` header.
fn content_range_start(header: &str) -> Option<u64> {
    header.strip_prefix("bytes ")?.split('-').next()?.trim().parse().ok()
}

/// Download every file of `spec` under `models_dir` (in the model's own folder, if it has
/// one). Files already installed at the right size are skipped; one progress bar covers the
/// whole model. If a file fails, the files already finished are kept for the next try.
/// `cancel` is checked continuously; set it from another thread to stop (partial files are kept).
pub fn download(
    spec: &ModelSpec,
    models_dir: &Path,
    options: &Options,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    let dir = spec.install_dir(models_dir);
    let total = spec.total_bytes();
    let mut finished = 0;
    for file in &spec.files {
        download_file(file, &dir, options, cancel, &mut |p| match p {
            Progress::Downloading { downloaded, .. } => on_progress(Progress::Downloading { downloaded: finished + downloaded, total }),
            Progress::Verifying => on_progress(Progress::Verifying),
            Progress::Done => {}
        })?;
        finished += file.size_bytes;
    }
    on_progress(Progress::Done);
    Ok(())
}

fn download_file(
    file: &ModelFile,
    dir: &Path,
    options: &Options,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    std::fs::create_dir_all(dir).map_err(DownloadError::from_io)?;
    let final_path = dir.join(&file.name);
    let part_path = dir.join(format!("{}.part", file.name));

    // Already installed at the right size: nothing to do.
    if std::fs::metadata(&final_path).is_ok_and(|m| m.len() == file.size_bytes) {
        on_progress(Progress::Downloading { downloaded: file.size_bytes, total: file.size_bytes });
        on_progress(Progress::Done);
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| DownloadError::Io(e.to_string()))?;
    runtime.block_on(fetch(file, &part_path, options, cancel, on_progress))?;

    on_progress(Progress::Verifying);
    let actual = sha256_of(&part_path).map_err(DownloadError::from_io)?;
    if actual != file.sha256.to_lowercase() {
        let _ = std::fs::remove_file(&part_path);
        return Err(DownloadError::HashMismatch { expected: file.sha256.clone(), actual });
    }
    std::fs::rename(&part_path, &final_path).map_err(DownloadError::from_io)?;
    on_progress(Progress::Done);
    Ok(())
}

async fn fetch(
    spec: &ModelFile,
    part: &Path,
    options: &Options,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > spec.size_bytes {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    if have == spec.size_bytes {
        return Ok(()); // fully downloaded earlier; only the check is left
    }
    let client = reqwest::Client::builder()
        .connect_timeout(options.connect_timeout)
        .build()
        .map_err(|e| DownloadError::Io(e.to_string()))?;

    // Up to two tries: the second starts from scratch when the server won't resume us.
    for _ in 0..2 {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let mut request = client.get(&spec.url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let response = request.send().await.map_err(from_send)?;
        match response.status().as_u16() {
            206 => {
                let start = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(content_range_start);
                if start != Some(have) {
                    let _ = std::fs::remove_file(part);
                    have = 0;
                    continue;
                }
            }
            // The server ignored the range and is sending the whole file.
            200 => have = 0,
            // Our partial file doesn't fit what the server has.
            416 => {
                let _ = std::fs::remove_file(part);
                have = 0;
                continue;
            }
            404 => return Err(DownloadError::NotFound),
            status => return Err(DownloadError::Http(status)),
        }
        return stream(response, spec.size_bytes, part, have, options, cancel, on_progress).await;
    }
    Err(DownloadError::Io("the server wouldn't send the file".into()))
}

async fn stream(
    mut response: reqwest::Response,
    total: u64,
    part: &Path,
    start: u64,
    options: &Options,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    let mut file: File = if start > 0 {
        OpenOptions::new().append(true).open(part)
    } else {
        File::create(part)
    }
    .map_err(DownloadError::from_io)?;

    let mut downloaded = start;
    on_progress(Progress::Downloading { downloaded, total });
    let mut last_data = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        // A short timeout around each read, so cancelling and detecting a stall don't have to
        // wait for the connection. Dropping an unfinished read loses no data.
        match tokio::time::timeout(POLL, response.chunk()).await {
            Ok(Ok(Some(bytes))) => {
                file.write_all(&bytes).map_err(DownloadError::from_io)?;
                downloaded += bytes.len() as u64;
                last_data = Instant::now();
                on_progress(Progress::Downloading { downloaded, total });
            }
            Ok(Ok(None)) => break,
            // The connection dropped part-way: keep what arrived.
            Ok(Err(_)) => return Err(DownloadError::Truncated { got: downloaded, expected: total }),
            Err(_idle) => {
                if last_data.elapsed() >= options.stall_timeout {
                    return Err(DownloadError::Stalled);
                }
            }
        }
    }
    file.flush().map_err(DownloadError::from_io)?;
    if downloaded != total {
        return Err(DownloadError::Truncated { got: downloaded, expected: total });
    }
    Ok(())
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
