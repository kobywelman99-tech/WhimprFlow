//! The downloader against a small HTTP server that cuts connections, ignores Range,
//! hangs, trickles and 404s on demand.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use whimpr_models::{download, DownloadError, ModelFile, ModelSpec, Options, Progress, Role};

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    /// The first request sends this many bytes of the body, then the connection drops.
    CutFirstAt(usize),
    /// Always answers 200 with the whole file, even when asked for a range.
    IgnoreRange,
    NotFound,
    /// Sends the body in pieces of `chunk` bytes with `gap_ms` between them.
    Trickle { chunk: usize, gap_ms: u64 },
    /// Sends this many bytes, then goes silent without closing.
    HangAfter(usize),
}

/// The path and `Range` start of each request the server has seen, in order.
type Hits = Mutex<Vec<(String, Option<u64>)>>;

struct Server {
    /// `http://127.0.0.1:port`, no trailing slash.
    base: String,
    /// The path and `Range` start of each request, in order.
    hits: Arc<Hits>,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The `Range` start of each request, in order (None when the request had no range).
    fn ranges(&self) -> Vec<Option<u64>> {
        self.hits.lock().unwrap().iter().map(|(_, r)| *r).collect()
    }

    fn requests_for(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|(p, _)| p == path).count()
    }
}

fn payload(len: usize) -> Arc<Vec<u8>> {
    // Deterministic, not repetitive, so a wrong splice changes the hash.
    let mut x = 0x2545_F491u32;
    Arc::new((0..len).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; (x >> 8) as u8 }).collect())
}

/// One file at `/model.bin`.
fn serve(body: Arc<Vec<u8>>, mode: Mode) -> Server {
    serve_files(vec![("/model.bin".to_string(), body)], mode)
}

fn serve_files(files: Vec<(String, Arc<Vec<u8>>)>, mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(Mutex::new(Vec::new()));
    let seen = hits.clone();
    let files = Arc::new(files);
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (files, seen) = (files.clone(), seen.clone());
            std::thread::spawn(move || handle(conn, &files, mode, &seen));
        }
    });
    Server { base, hits }
}

fn handle(mut conn: TcpStream, files: &[(String, Arc<Vec<u8>>)], mode: Mode, seen: &Hits) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match conn.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&request).to_lowercase();
    let range = text.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.trim_end_matches('-').trim().parse::<u64>().ok());
    let path = String::from_utf8_lossy(&request).split_whitespace().nth(1).unwrap_or("/").to_string();
    let first = {
        let mut seen = seen.lock().unwrap();
        seen.push((path.clone(), range));
        seen.len() == 1
    };
    let Some(body) = files.iter().find(|(p, _)| *p == path).map(|(_, b)| b.as_slice()) else {
        let _ = conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };

    if matches!(mode, Mode::NotFound) {
        let _ = conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let start = if matches!(mode, Mode::IgnoreRange) { 0 } else { range.unwrap_or(0) as usize };
    let rest = &body[start.min(body.len())..];
    let head = if start > 0 {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len() - 1,
            body.len(),
            rest.len()
        )
    } else {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", rest.len())
    };
    let _ = conn.write_all(head.as_bytes());
    match mode {
        Mode::CutFirstAt(n) if first => {
            let _ = conn.write_all(&rest[..n.min(rest.len())]);
            let _ = conn.flush();
            let _ = conn.shutdown(Shutdown::Both);
        }
        Mode::HangAfter(n) => {
            let _ = conn.write_all(&rest[..n.min(rest.len())]);
            let _ = conn.flush();
            std::thread::sleep(Duration::from_secs(30));
        }
        Mode::Trickle { chunk, gap_ms } => {
            for piece in rest.chunks(chunk) {
                if conn.write_all(piece).is_err() {
                    return;
                }
                let _ = conn.flush();
                std::thread::sleep(Duration::from_millis(gap_ms));
            }
        }
        _ => {
            let _ = conn.write_all(rest);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn file(name: &str, url: &str, body: &[u8]) -> ModelFile {
    ModelFile { name: name.into(), url: url.into(), size_bytes: body.len() as u64, sha256: hex(body) }
}

/// A one-file model, `model.bin`, straight in the models folder (like a whisper model).
fn spec(server_url: &str, body: &[u8]) -> ModelSpec {
    ModelSpec {
        id: "test".into(),
        title: "Test".into(),
        blurb: String::new(),
        role: Role::Speech,
        folder: None,
        files: vec![file("model.bin", server_url, body)],
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("whimpr-models-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn options() -> Options {
    Options { stall_timeout: Duration::from_millis(800), connect_timeout: Duration::from_secs(2) }
}

fn run(spec: &ModelSpec, dir: &Path, cancel: &AtomicBool) -> (Result<(), DownloadError>, Vec<Progress>) {
    let mut events = Vec::new();
    let result = download(spec, dir, &options(), cancel, &mut |p| events.push(p));
    (result, events)
}

fn part(dir: &Path) -> PathBuf {
    dir.join("model.bin.part")
}

#[test]
fn a_download_arrives_verified_with_no_leftovers() {
    let body = payload(600_000);
    let server = serve(body.clone(), Mode::Normal);
    let dir = temp_dir("full");

    let (result, events) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));

    result.expect("downloads");
    assert_eq!(std::fs::read(dir.join("model.bin")).unwrap(), *body);
    assert!(!part(&dir).exists(), "the .part file is gone");
    // Progress only goes up, ends at the total, then reports verifying and done.
    let downloaded: Vec<u64> = events.iter().filter_map(|e| match e { Progress::Downloading { downloaded, .. } => Some(*downloaded), _ => None }).collect();
    assert!(downloaded.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(downloaded.last(), Some(&(body.len() as u64)));
    assert!(matches!(events[events.len() - 2], Progress::Verifying));
    assert!(matches!(events[events.len() - 1], Progress::Done));
}

#[test]
fn a_dropped_connection_keeps_the_partial_file_and_the_next_try_resumes_from_it() {
    let body = payload(600_000);
    let server = serve(body.clone(), Mode::CutFirstAt(250_000));
    let dir = temp_dir("resume");
    let spec = spec(&server.url("/model.bin"), &body);

    let (first, _) = run(&spec, &dir, &AtomicBool::new(false));
    assert!(matches!(first, Err(DownloadError::Truncated { got: 250_000, .. })), "{first:?}");
    assert_eq!(std::fs::metadata(part(&dir)).unwrap().len(), 250_000, "what arrived is kept");
    assert!(!dir.join("model.bin").exists(), "nothing the app could load yet");

    let (second, events) = run(&spec, &dir, &AtomicBool::new(false));
    second.expect("resumes and finishes");
    assert_eq!(std::fs::read(dir.join("model.bin")).unwrap(), *body, "the splice is byte-exact");
    assert_eq!(server.ranges().as_slice(), [None, Some(250_000)], "the second request asked only for the rest");
    assert!(matches!(events[0], Progress::Downloading { downloaded: 250_000, .. }), "progress starts from what it already had");
}

#[test]
fn a_file_that_does_not_match_its_hash_is_thrown_away() {
    let body = payload(300_000);
    let server = serve(body.clone(), Mode::Normal);
    let dir = temp_dir("hash");
    let mut spec = spec(&server.url("/model.bin"), &body);
    spec.files[0].sha256 = hex(b"something else entirely");

    let (result, _) = run(&spec, &dir, &AtomicBool::new(false));

    assert!(matches!(result, Err(DownloadError::HashMismatch { .. })), "{result:?}");
    assert!(!dir.join("model.bin").exists() && !part(&dir).exists(), "a bad file must not be left where it could be loaded or resumed");
}

#[test]
fn a_server_that_ignores_the_range_request_still_gives_a_correct_file() {
    let body = payload(400_000);
    let server = serve(body.clone(), Mode::IgnoreRange);
    let dir = temp_dir("norange");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(part(&dir), &body[..100_000]).unwrap();

    let (result, _) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));

    result.expect("restarts from the beginning");
    assert_eq!(std::fs::read(dir.join("model.bin")).unwrap(), *body);
}

#[test]
fn a_missing_file_is_reported_as_not_found_and_leaves_nothing() {
    let body = payload(1000);
    let server = serve(body.clone(), Mode::NotFound);
    let dir = temp_dir("404");
    let (result, _) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));
    assert!(matches!(result, Err(DownloadError::NotFound)), "{result:?}");
    assert!(!dir.join("model.bin").exists() && !part(&dir).exists());
}

#[test]
fn cancelling_stops_promptly_and_keeps_the_partial_file_for_later() {
    let body = payload(600_000);
    let server = serve(body.clone(), Mode::Trickle { chunk: 30_000, gap_ms: 25 });
    let dir = temp_dir("cancel");
    let cancel = AtomicBool::new(false);
    let spec = spec(&server.url("/model.bin"), &body);

    let mut events = 0;
    let result = download(&spec, &dir, &options(), &cancel, &mut |p| {
        if let Progress::Downloading { downloaded, .. } = p {
            events += 1;
            if downloaded >= 100_000 {
                cancel.store(true, Ordering::SeqCst);
            }
        }
    });

    assert!(matches!(result, Err(DownloadError::Cancelled)), "{result:?}");
    let kept = std::fs::metadata(part(&dir)).unwrap().len();
    assert!((100_000..600_000).contains(&kept), "kept {kept}");
    assert!(events < 20, "kept downloading after the cancel: {events} events");

    // Later, it picks up where it stopped.
    let (resumed, _) = run(&spec, &dir, &AtomicBool::new(false));
    resumed.expect("resumes");
    assert_eq!(std::fs::read(dir.join("model.bin")).unwrap(), *body);
}

#[test]
fn a_slow_trickle_is_not_mistaken_for_a_stall_and_loses_no_data() {
    // Gaps longer than the 250 ms polling interval, so the poll gives up and retries between pieces.
    let body = payload(40_000);
    let server = serve(body.clone(), Mode::Trickle { chunk: 8_000, gap_ms: 400 });
    let dir = temp_dir("trickle");
    let (result, _) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));
    result.expect("completes");
    assert_eq!(std::fs::read(dir.join("model.bin")).unwrap(), *body, "byte-exact despite the polling");
}

#[test]
fn a_connection_that_goes_silent_is_reported_as_stalled() {
    let body = payload(400_000);
    let server = serve(body.clone(), Mode::HangAfter(120_000));
    let dir = temp_dir("stall");
    let began = Instant::now();
    let (result, _) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));
    assert!(matches!(result, Err(DownloadError::Stalled)), "{result:?}");
    assert!(began.elapsed() < Duration::from_secs(5), "took {:?}", began.elapsed());
    assert!(std::fs::metadata(part(&dir)).unwrap().len() >= 120_000, "what arrived is kept");
}

#[test]
fn an_already_installed_model_is_not_downloaded_again() {
    let body = payload(5_000);
    let server = serve(body.clone(), Mode::Normal);
    let dir = temp_dir("present");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.bin"), &*body).unwrap();

    let (result, events) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));

    result.expect("already there");
    assert!(server.ranges().is_empty(), "no request was made");
    assert!(matches!(events.last(), Some(Progress::Done)));
}

#[test]
fn a_leftover_that_is_bigger_than_the_model_is_discarded_not_trusted() {
    let body = payload(50_000);
    let server = serve(body.clone(), Mode::Normal);
    let dir = temp_dir("oversize");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(part(&dir), vec![7u8; 80_000]).unwrap();
    let (result, _) = run(&spec(&server.url("/model.bin"), &body), &dir, &AtomicBool::new(false));
    result.expect("starts over");
    assert_eq!(std::fs::read(dir.join("model.bin")).unwrap(), *body);
}

#[test]
fn a_server_that_is_not_there_is_reported_as_offline() {
    let dir = temp_dir("offline");
    // Bind then drop, so the port is closed.
    let url = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); format!("http://{}/model.bin", l.local_addr().unwrap()) };
    let (result, _) = run(&spec(&url, b"x"), &dir, &AtomicBool::new(false));
    assert!(matches!(result, Err(DownloadError::Offline(_))), "{result:?}");
}

#[test]
fn error_messages_are_written_for_the_person_not_the_programmer() {
    for (error, must_mention) in [
        (DownloadError::Offline("x".into()), "internet"),
        (DownloadError::NotFound, "no longer"),
        (DownloadError::Stalled, "stopped"),
        (DownloadError::DiskFull, "space"),
        (DownloadError::Cancelled, "cancel"),
        (DownloadError::HashMismatch { expected: "a".into(), actual: "b".into() }, "damaged"),
        (DownloadError::Truncated { got: 1, expected: 2 }, "try again"),
    ] {
        let text = error.to_string().to_lowercase();
        assert!(text.contains(must_mention), "{error:?} -> {text}");
    }
    // A full disk is recognised from the OS error, not only from our own messages.
    let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
    assert!(matches!(DownloadError::from_io(full), DownloadError::DiskFull));
}

// ── Models made of several files (Parakeet: encoder, decoder, preprocessor, vocabulary) ──

fn multi(server: &Server, files: &[(&str, &Arc<Vec<u8>>)]) -> ModelSpec {
    ModelSpec {
        id: "multi".into(),
        title: "Multi".into(),
        blurb: String::new(),
        role: Role::Speech,
        folder: Some("multi-model".into()),
        files: files.iter().map(|(name, body)| file(name, &server.url(&format!("/{name}")), body)).collect(),
    }
}

#[test]
fn a_model_of_several_files_downloads_into_its_own_folder_with_one_progress_bar() {
    let (a, b, c) = (payload(300_000), payload(50_000), payload(2_000));
    let server = serve_files(
        vec![("/a.onnx".into(), a.clone()), ("/b.onnx".into(), b.clone()), ("/vocab.txt".into(), c.clone())],
        Mode::Normal,
    );
    let dir = temp_dir("multi");
    let spec = multi(&server, &[("a.onnx", &a), ("b.onnx", &b), ("vocab.txt", &c)]);
    assert_eq!(spec.total_bytes(), 352_000);

    let (result, events) = run(&spec, &dir, &AtomicBool::new(false));
    result.expect("downloads all three");

    for (name, body) in [("a.onnx", &a), ("b.onnx", &b), ("vocab.txt", &c)] {
        assert_eq!(std::fs::read(dir.join("multi-model").join(name)).unwrap(), **body, "{name}");
    }
    assert!(spec.is_installed(&dir));
    // One bar across all files: totals are the whole model, progress never goes backwards and ends at the total.
    let bars: Vec<(u64, u64)> = events.iter().filter_map(|e| match e { Progress::Downloading { downloaded, total } => Some((*downloaded, *total)), _ => None }).collect();
    assert!(bars.iter().all(|(_, total)| *total == 352_000));
    assert!(bars.windows(2).all(|w| w[0].0 <= w[1].0));
    assert_eq!(bars.last().unwrap().0, 352_000);
    assert!(matches!(events.last(), Some(Progress::Done)));
    assert_eq!(events.iter().filter(|e| matches!(e, Progress::Done)).count(), 1, "done is reported once, at the end");
}

#[test]
fn files_that_are_already_there_are_not_fetched_again_and_the_rest_are() {
    let (a, b) = (payload(40_000), payload(30_000));
    let server = serve_files(vec![("/a.onnx".into(), a.clone()), ("/b.onnx".into(), b.clone())], Mode::Normal);
    let dir = temp_dir("multi-partial");
    std::fs::create_dir_all(dir.join("multi-model")).unwrap();
    std::fs::write(dir.join("multi-model").join("a.onnx"), &*a).unwrap();
    let spec = multi(&server, &[("a.onnx", &a), ("b.onnx", &b)]);
    assert!(!spec.is_installed(&dir), "one of two is not a model");

    let (result, events) = run(&spec, &dir, &AtomicBool::new(false));

    result.expect("fetches the missing one");
    assert_eq!(server.requests_for("/a.onnx"), 0, "the file that was already there");
    assert_eq!(server.requests_for("/b.onnx"), 1);
    assert!(spec.is_installed(&dir));
    // The bar starts with the already-present bytes counted, not from zero.
    assert!(matches!(events[0], Progress::Downloading { downloaded: 40_000, total: 70_000 }), "{:?}", events[0]);
}

#[test]
fn if_one_file_fails_the_model_is_not_installed_but_the_others_are_kept_for_next_time() {
    let (a, b) = (payload(40_000), payload(30_000));
    // The server has a.onnx but not b.onnx.
    let server = serve_files(vec![("/a.onnx".into(), a.clone())], Mode::Normal);
    let dir = temp_dir("multi-fail");
    let spec = multi(&server, &[("a.onnx", &a), ("b.onnx", &b)]);

    let (result, _) = run(&spec, &dir, &AtomicBool::new(false));

    assert!(matches!(result, Err(DownloadError::NotFound)), "{result:?}");
    assert!(!spec.is_installed(&dir), "half a model must not count as installed");
    assert!(dir.join("multi-model").join("a.onnx").exists(), "the finished file is kept");
}

#[test]
fn a_model_is_installed_only_when_every_file_has_the_right_size() {
    let server = serve_files(vec![], Mode::Normal);
    let (a, b) = (payload(100), payload(50));
    let spec = multi(&server, &[("a.onnx", &a), ("b.onnx", &b)]);
    let dir = temp_dir("installed-check");
    let folder = dir.join("multi-model");
    std::fs::create_dir_all(&folder).unwrap();
    assert!(!spec.is_installed(&dir));
    std::fs::write(folder.join("a.onnx"), &*a).unwrap();
    assert!(!spec.is_installed(&dir));
    std::fs::write(folder.join("b.onnx"), vec![0u8; 10]).unwrap(); // truncated
    assert!(!spec.is_installed(&dir), "a truncated file doesn't count");
    std::fs::write(folder.join("b.onnx"), &*b).unwrap();
    assert!(spec.is_installed(&dir));
}
