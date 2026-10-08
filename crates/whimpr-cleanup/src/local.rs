//! The on-device cleanup model, run as a separate process (llama.cpp and whisper.cpp must
//! not share a binary) and supervised so it can never freeze a dictation.
//!
//! - Every request has a hard timeout.
//! - A hang, crash or unreadable reply kills the process and starts a fresh one in the
//!   background. The request that hit the problem fails at once, so the caller pastes the
//!   gated raw transcript instead of waiting.
//! - While the model is loading (it takes seconds), requests fail immediately instead of
//!   queueing behind the load.
//! - After loading, one tiny request is sent so the first real dictation isn't cold.
//!
//! The protocol is one JSON object per line over stdio. The worker prints `{"ready":true}`
//! once its model is loaded, then answers each `{"messages":[…],"max_tokens":N}` with
//! `{"text":"…"}` or `{"text":"","error":"…"}`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use whimpr_core::cleanup::{CleanupMsg, CleanupProvider, ProviderId};

#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("the on-device model isn't ready")]
    NotReady,
    #[error("the on-device model took longer than {0:?}")]
    Timeout(Duration),
    #[error("the on-device model stopped")]
    Exited,
    #[error("the on-device model sent something unreadable: {0}")]
    BadResponse(String),
    #[error("local llm: {0}")]
    Worker(String),
}

impl LocalError {
    /// Whether the process can no longer be trusted and must be replaced.
    fn needs_restart(&self) -> bool {
        matches!(self, Self::Timeout(_) | Self::Exited | Self::BadResponse(_))
    }
}

#[derive(Debug, Clone)]
pub struct LocalConfig {
    pub bin: PathBuf,
    /// Passed as the worker's first argument.
    pub model: PathBuf,
    /// Longest a single request may take.
    pub timeout: Duration,
    /// Longest the model may take to load.
    pub ready_timeout: Duration,
    /// Send one tiny request after loading, so the first real one isn't cold.
    pub warm: bool,
    /// After a worker fails to start, wait this long before trying again.
    pub retry_after_failed_start: Duration,
}

impl LocalConfig {
    pub fn new(bin: PathBuf, model: PathBuf) -> Self {
        Self {
            bin,
            model,
            timeout: Duration::from_millis(3000),
            ready_timeout: Duration::from_secs(90),
            warm: true,
            retry_after_failed_start: Duration::from_secs(5),
        }
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Worker {
    /// Start the process and wait until it says its model is loaded (and warm it).
    fn launch(cfg: &LocalConfig) -> anyhow::Result<Self> {
        let mut child = Command::new(&cfg.bin)
            .arg(&cfg.model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;

        // A reader thread turns the blocking pipe into something with a timeout. It ends
        // when the process does.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut worker = Worker { child, stdin, lines };

        match worker.lines.recv_timeout(cfg.ready_timeout) {
            Ok(line) if line.contains("\"ready\"") => {}
            Ok(line) => anyhow::bail!("expected a ready message, got {line:?}"),
            Err(RecvTimeoutError::Timeout) => anyhow::bail!("the model didn't load within {:?}", cfg.ready_timeout),
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("the worker exited while loading"),
        }
        if cfg.warm {
            let warm = [CleanupMsg { role: "user", content: "Reply with: ok".into() }];
            // A failed warm-up isn't fatal, but a hung one means a broken worker.
            if let Err(e @ (LocalError::Timeout(_) | LocalError::Exited)) = worker.roundtrip(&warm, cfg.ready_timeout, 4) {
                anyhow::bail!("warm-up failed: {e}");
            }
        }
        Ok(worker)
    }

    fn roundtrip(&mut self, messages: &[CleanupMsg], timeout: Duration, max_tokens: u32) -> Result<String, LocalError> {
        let mut line = serde_json::to_string(&serde_json::json!({ "messages": messages, "max_tokens": max_tokens }))
            .map_err(|e| LocalError::BadResponse(e.to_string()))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|_| LocalError::Exited)?;

        let reply = match self.lines.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => return Err(LocalError::Timeout(timeout)),
            Err(RecvTimeoutError::Disconnected) => return Err(LocalError::Exited),
        };
        let v: serde_json::Value = serde_json::from_str(&reply).map_err(|_| LocalError::BadResponse(reply.chars().take(80).collect()))?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()).filter(|e| !e.is_empty()) {
            return Err(LocalError::Worker(err.to_string()));
        }
        Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum State {
    /// Not running. `retry_at` is set after a failed start, to avoid a restart storm.
    Stopped { retry_at: Option<Instant> },
    Loading,
    Ready(Worker),
}

struct Shared {
    cfg: LocalConfig,
    timeout_ms: AtomicU64,
    state: Mutex<State>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The supervised on-device model. Cheap to share; all methods take `&self`.
pub struct LocalLlm {
    shared: Arc<Shared>,
}

impl LocalLlm {
    /// Create the supervisor. Nothing starts until [`start`](Self::start).
    pub fn new(cfg: LocalConfig) -> Self {
        let timeout_ms = AtomicU64::new(cfg.timeout.as_millis() as u64);
        Self { shared: Arc::new(Shared { cfg, timeout_ms, state: Mutex::new(State::Stopped { retry_at: None }) }) }
    }

    /// Change the per-request timeout (takes effect on the next request).
    pub fn set_timeout(&self, timeout: Duration) {
        self.shared.timeout_ms.store(timeout.as_millis() as u64, Ordering::Relaxed);
    }

    /// Start loading in the background, if it isn't running or loading already. Safe to call
    /// at launch, when Local mode is chosen, and whenever it might have died.
    pub fn start(&self) {
        {
            let mut state = lock(&self.shared.state);
            match &*state {
                State::Stopped { retry_at } if retry_at.is_none_or(|t| Instant::now() >= t) => *state = State::Loading,
                _ => return,
            }
        }
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let launched = Worker::launch(&shared.cfg);
            let mut state = lock(&shared.state);
            *state = match launched {
                Ok(worker) => {
                    eprintln!("[whimpr] on-device model ready");
                    State::Ready(worker)
                }
                Err(e) => {
                    eprintln!("[whimpr] on-device model failed to start: {e}");
                    State::Stopped { retry_at: Some(Instant::now() + shared.cfg.retry_after_failed_start) }
                }
            };
        });
    }

    /// Whether a request would be accepted right now. A request in flight counts as ready.
    pub fn is_ready(&self) -> bool {
        match self.shared.state.try_lock() {
            Ok(state) => matches!(*state, State::Ready(_)),
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Poisoned(p)) => matches!(*p.into_inner(), State::Ready(_)),
        }
    }

    fn request(&self, messages: &[CleanupMsg]) -> Result<String, LocalError> {
        let timeout = Duration::from_millis(self.shared.timeout_ms.load(Ordering::Relaxed));
        let mut state = lock(&self.shared.state);
        let result = match &mut *state {
            State::Ready(worker) => worker.roundtrip(messages, timeout, 400),
            State::Loading => return Err(LocalError::NotReady),
            State::Stopped { .. } => {
                drop(state);
                self.start();
                return Err(LocalError::NotReady);
            }
        };
        if let Err(e) = &result {
            if e.needs_restart() {
                eprintln!("[whimpr] on-device model failed ({e}); restarting it");
                // Dropping the worker kills the process.
                *state = State::Stopped { retry_at: None };
                drop(state);
                self.start();
            }
        }
        result
    }
}

impl CleanupProvider for LocalLlm {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }

    fn complete(&self, messages: &[CleanupMsg]) -> anyhow::Result<String> {
        Ok(self.request(messages)?)
    }
}
