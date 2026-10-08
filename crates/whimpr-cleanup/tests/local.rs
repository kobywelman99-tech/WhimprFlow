//! The supervisor against a fake worker that hangs, crashes, lies and dawdles on demand.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use whimpr_cleanup::{LocalConfig, LocalError, LocalLlm};
use whimpr_core::cleanup::{CleanupMsg, CleanupProvider};

fn config(mode: &str) -> LocalConfig {
    let mut cfg = LocalConfig::new(PathBuf::from(env!("CARGO_BIN_EXE_fake-llm-worker")), PathBuf::from(mode));
    cfg.warm = false;
    cfg.timeout = Duration::from_millis(400);
    cfg.ready_timeout = Duration::from_secs(5);
    cfg.retry_after_failed_start = Duration::from_millis(300);
    cfg
}

fn messages() -> Vec<CleanupMsg> {
    vec![CleanupMsg { role: "user", content: "hello".into() }]
}

fn marker(tag: &str) -> String {
    let path = std::env::temp_dir().join(format!("whimpr-local-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

fn lines_in(path: &str) -> usize {
    std::fs::read_to_string(path).map_or(0, |s| s.lines().count())
}

fn wait_until(what: &str, deadline: Duration, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn started(cfg: LocalConfig) -> LocalLlm {
    let llm = LocalLlm::new(cfg);
    llm.start();
    wait_until("the worker to be ready", Duration::from_secs(5), || llm.is_ready());
    llm
}

fn error_of(llm: &LocalLlm) -> LocalError {
    match llm.complete(&messages()) {
        Err(e) => e.downcast::<LocalError>().expect("a LocalError"),
        Ok(text) => panic!("expected an error, got {text:?}"),
    }
}

#[test]
fn a_healthy_worker_answers() {
    let llm = started(config("ok"));
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
    assert_eq!(llm.complete(&messages()).unwrap(), "OK", "and keeps answering");
}

#[test]
fn a_slow_but_working_worker_is_waited_for() {
    let llm = started(config("slow:150"));
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}

#[test]
fn a_hung_worker_times_out_on_time_and_the_next_request_succeeds_after_a_restart() {
    let flag = marker("hang");
    let llm = started(config(&format!("hang-once:{flag}")));

    let began = Instant::now();
    assert!(matches!(error_of(&llm), LocalError::Timeout(_)));
    let took = began.elapsed();
    assert!(took >= Duration::from_millis(350) && took < Duration::from_millis(1500), "returned after {took:?}");

    // A fresh process is started in the background and works.
    wait_until("the restarted worker", Duration::from_secs(5), || llm.is_ready());
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}

#[test]
fn a_worker_that_exits_fails_the_request_immediately_and_is_replaced() {
    let flag = marker("exit");
    let mut cfg = config(&format!("exit-once:{flag}"));
    cfg.timeout = Duration::from_secs(10); // a long timeout must not be what ends this
    let llm = started(cfg);

    let began = Instant::now();
    assert!(matches!(error_of(&llm), LocalError::Exited));
    assert!(began.elapsed() < Duration::from_secs(2), "waited {:?} for a dead worker", began.elapsed());

    wait_until("the restarted worker", Duration::from_secs(5), || llm.is_ready());
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}

#[test]
fn unreadable_output_fails_the_request_and_replaces_the_worker() {
    let flag = marker("garbage");
    let llm = started(config(&format!("garbage-once:{flag}")));
    assert!(matches!(error_of(&llm), LocalError::BadResponse(_)));
    wait_until("the restarted worker", Duration::from_secs(5), || llm.is_ready());
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}

#[test]
fn while_the_model_is_loading_a_request_fails_at_once_instead_of_waiting() {
    let llm = LocalLlm::new(config("slow-ready:1500"));
    llm.start();
    std::thread::sleep(Duration::from_millis(100));

    let began = Instant::now();
    assert!(matches!(error_of(&llm), LocalError::NotReady));
    assert!(began.elapsed() < Duration::from_millis(100), "blocked for {:?}", began.elapsed());
    assert!(!llm.is_ready());

    wait_until("the model to finish loading", Duration::from_secs(5), || llm.is_ready());
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}

#[test]
fn a_request_to_a_stopped_worker_starts_it() {
    let llm = LocalLlm::new(config("ok"));
    assert!(matches!(error_of(&llm), LocalError::NotReady), "nothing was started yet");
    wait_until("the worker to start itself", Duration::from_secs(5), || llm.is_ready());
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}

#[test]
fn the_model_is_warmed_before_it_counts_as_ready() {
    let log = marker("count");
    let mut cfg = config(&format!("count:{log}"));
    cfg.warm = true;
    let llm = started(cfg);
    assert_eq!(lines_in(&log), 1, "one warm-up request, sent before any real one");
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
    assert_eq!(lines_in(&log), 2);
}

#[test]
fn a_worker_that_keeps_failing_to_start_is_not_respawned_in_a_loop() {
    let log = marker("crash");
    let mut cfg = config(&format!("crash-on-start:{log}"));
    cfg.retry_after_failed_start = Duration::from_secs(30);
    let llm = LocalLlm::new(cfg);
    llm.start();
    wait_until("the first attempt", Duration::from_secs(5), || lines_in(&log) >= 1);
    std::thread::sleep(Duration::from_millis(200));

    for _ in 0..5 {
        assert!(matches!(error_of(&llm), LocalError::NotReady));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(lines_in(&log), 1, "it must wait out the retry delay, not spawn on every request");
}

#[test]
fn a_worker_that_never_finishes_loading_gives_up() {
    let mut cfg = config("never-ready");
    cfg.ready_timeout = Duration::from_millis(300);
    let llm = LocalLlm::new(cfg);
    llm.start();
    std::thread::sleep(Duration::from_millis(900));
    assert!(!llm.is_ready());
    assert!(matches!(error_of(&llm), LocalError::NotReady));
}

#[test]
fn the_timeout_can_be_changed_while_running() {
    let llm = started(config("slow:300"));
    llm.set_timeout(Duration::from_millis(100));
    assert!(matches!(error_of(&llm), LocalError::Timeout(_)));
    wait_until("the restarted worker", Duration::from_secs(5), || llm.is_ready());
    llm.set_timeout(Duration::from_secs(2));
    assert_eq!(llm.complete(&messages()).unwrap(), "OK");
}
