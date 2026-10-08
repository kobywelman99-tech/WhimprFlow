//! The downloader against a small HTTP server that cuts connections, ignores Range,
//! hangs, trickles and 404s on demand.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use whimpr_models::{download, DownloadError, ModelSpec, Options, Progress, Role};

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

struct Server {
    url: String,
    /// The `Range` start of each request, in order (None when the request had no range).
    ranges: Arc<Mutex<Vec<Option<u64>>>>,
}

fn payload(len: usize) -> Arc<Vec<u8>> {
    // Deterministic, not repetitive, so a wrong splice changes the hash.
    let mut x = 0x2545_F491u32;
    Arc::new((0..len).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; (x >> 8) as u8 }).collect())
}

fn serve(body: Arc<Vec<u8>>, mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let seen = ranges.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (body, seen) = (body.clone(), seen.clone());
            std::thread::spawn(move || handle(conn, &body, mode, &seen));
        }
    });
    Server { url, ranges }
}

fn handle(mut conn: TcpStream, body: &[u8], mode: Mode, seen: &Mutex<Vec<Option<u64>>>) {
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
    let first = {
        let mut seen = seen.lock().unwrap();
        seen.push(range);
        seen.len() == 1
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

fn spec(server_url: &str, body: &[u8]) -> ModelSpec {
    ModelSpec {
        id: "test".into(),
        title: "Test".into(),
        blurb: String::new(),
        file_name: "model.bin".into(),
        url: server_url.into(),
        size_bytes: body.len() as u64,
        sha256: hex(body),
        role: Role::Speech,
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

fn run(spec: &ModelSpec, dir: &Path, cancel: &AtomicBool) -> (Result<PathBuf, DownloadError>, Vec<Progress>) {
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

    let (result, events) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));

    let path = result.expect("downloads");
    assert_eq!(std::fs::read(&path).unwrap(), *body);
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
    let spec = spec(&server.url, &body);

    let (first, _) = run(&spec, &dir, &AtomicBool::new(false));
    assert!(matches!(first, Err(DownloadError::Truncated { got: 250_000, .. })), "{first:?}");
    assert_eq!(std::fs::metadata(part(&dir)).unwrap().len(), 250_000, "what arrived is kept");
    assert!(!dir.join("model.bin").exists(), "nothing the app could load yet");

    let (second, events) = run(&spec, &dir, &AtomicBool::new(false));
    let path = second.expect("resumes and finishes");
    assert_eq!(std::fs::read(path).unwrap(), *body, "the splice is byte-exact");
    assert_eq!(server.ranges.lock().unwrap().as_slice(), [None, Some(250_000)], "the second request asked only for the rest");
    assert!(matches!(events[0], Progress::Downloading { downloaded: 250_000, .. }), "progress starts from what it already had");
}

#[test]
fn a_file_that_does_not_match_its_hash_is_thrown_away() {
    let body = payload(300_000);
    let server = serve(body.clone(), Mode::Normal);
    let dir = temp_dir("hash");
    let mut spec = spec(&server.url, &body);
    spec.sha256 = hex(b"something else entirely");

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

    let (result, _) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));

    assert_eq!(std::fs::read(result.expect("restarts from the beginning")).unwrap(), *body);
}

#[test]
fn a_missing_file_is_reported_as_not_found_and_leaves_nothing() {
    let body = payload(1000);
    let server = serve(body.clone(), Mode::NotFound);
    let dir = temp_dir("404");
    let (result, _) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));
    assert!(matches!(result, Err(DownloadError::NotFound)), "{result:?}");
    assert!(!dir.join("model.bin").exists() && !part(&dir).exists());
}

#[test]
fn cancelling_stops_promptly_and_keeps_the_partial_file_for_later() {
    let body = payload(600_000);
    let server = serve(body.clone(), Mode::Trickle { chunk: 30_000, gap_ms: 25 });
    let dir = temp_dir("cancel");
    let cancel = AtomicBool::new(false);
    let spec = spec(&server.url, &body);

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
    assert_eq!(std::fs::read(resumed.expect("resumes")).unwrap(), *body);
}

#[test]
fn a_slow_trickle_is_not_mistaken_for_a_stall_and_loses_no_data() {
    // Gaps longer than the 250 ms polling interval, so the poll gives up and retries between pieces.
    let body = payload(40_000);
    let server = serve(body.clone(), Mode::Trickle { chunk: 8_000, gap_ms: 400 });
    let dir = temp_dir("trickle");
    let (result, _) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));
    assert_eq!(std::fs::read(result.expect("completes")).unwrap(), *body, "byte-exact despite the polling");
}

#[test]
fn a_connection_that_goes_silent_is_reported_as_stalled() {
    let body = payload(400_000);
    let server = serve(body.clone(), Mode::HangAfter(120_000));
    let dir = temp_dir("stall");
    let began = Instant::now();
    let (result, _) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));
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

    let (result, events) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));

    assert_eq!(result.expect("already there"), dir.join("model.bin"));
    assert!(server.ranges.lock().unwrap().is_empty(), "no request was made");
    assert!(matches!(events.last(), Some(Progress::Done)));
}

#[test]
fn a_leftover_that_is_bigger_than_the_model_is_discarded_not_trusted() {
    let body = payload(50_000);
    let server = serve(body.clone(), Mode::Normal);
    let dir = temp_dir("oversize");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(part(&dir), vec![7u8; 80_000]).unwrap();
    let (result, _) = run(&spec(&server.url, &body), &dir, &AtomicBool::new(false));
    assert_eq!(std::fs::read(result.expect("starts over")).unwrap(), *body);
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
