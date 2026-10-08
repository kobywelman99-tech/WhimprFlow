//! Speech-model downloads for the Hub: what's installed, start/cancel a download, progress
//! events. The downloading itself is `whimpr-models`; this connects it to the app.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use whimpr_models::{ModelSpec, Progress};

/// The event the Hub listens to.
const PROGRESS_EVENT: &str = "whimpr://models/progress";

/// One row of the models list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelDto {
    pub id: String,
    pub title: String,
    pub blurb: String,
    pub size_bytes: u64,
    /// The file is in the models folder at the right size.
    pub installed: bool,
    /// This is the one speech recognition is using.
    pub active: bool,
    pub downloading: bool,
}

/// Describe `specs` for the UI. `active_file` is the model file in use, `downloading` the
/// id being fetched right now.
pub fn describe(specs: &[ModelSpec], dir: &Path, active_file: Option<&str>, downloading: Option<&str>) -> Vec<ModelDto> {
    specs
        .iter()
        .map(|s| ModelDto {
            id: s.id.clone(),
            title: s.title.clone(),
            blurb: s.blurb.clone(),
            size_bytes: s.size_bytes,
            installed: std::fs::metadata(dir.join(&s.file_name)).is_ok_and(|m| m.len() == s.size_bytes),
            active: active_file == Some(s.file_name.as_str()),
            downloading: downloading == Some(s.id.as_str()),
        })
        .collect()
}

/// A progress update for the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgressDto {
    pub id: String,
    /// `downloading`, `verifying`, `done`, `cancelled` or `error`.
    pub state: &'static str,
    pub downloaded: u64,
    pub total: u64,
    /// A message for the person: the error, or a note such as "restart to use it".
    pub message: Option<String>,
}

/// Lets at most one progress event through per interval, so a fast download doesn't flood
/// the UI. Events that change the state (not just the byte count) always pass.
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self { interval, last: None }
    }

    pub fn ready(&mut self, now: Instant, important: bool) -> bool {
        let due = important || self.last.is_none_or(|t| now.duration_since(t) >= self.interval);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// The download in progress, if any: its model id and the flag that cancels it.
static ACTIVE: Mutex<Option<(String, Arc<AtomicBool>)>> = Mutex::new(None);

fn active() -> std::sync::MutexGuard<'static, Option<(String, Arc<AtomicBool>)>> {
    ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Claim the single download slot for `id`. Fails if one is already running.
pub fn try_begin(id: &str) -> Result<Arc<AtomicBool>, String> {
    let mut slot = active();
    if let Some((running, _)) = slot.as_ref() {
        return Err(format!("Already downloading another model ({running}). Wait for it to finish or cancel it first."));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *slot = Some((id.to_string(), cancel.clone()));
    Ok(cancel)
}

fn finish() {
    *active() = None;
}

/// Ask the running download to stop. Returns whether there was one.
pub fn cancel() -> bool {
    match active().as_ref() {
        Some((_, flag)) => {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            true
        }
        None => false,
    }
}

pub fn downloading_id() -> Option<String> {
    active().as_ref().map(|(id, _)| id.clone())
}

pub fn list() -> Vec<ModelDto> {
    let dir = crate::hotkey::models_dir();
    let active_file = whimpr_models::best_whisper_model(&dir)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    describe(&whimpr_models::builtin(), &dir, active_file.as_deref(), downloading_id().as_deref())
}

/// Whether any speech model is installed (for the onboarding gate and diagnostics).
pub fn asr_model_installed() -> bool {
    whimpr_models::best_whisper_model(&crate::hotkey::models_dir()).is_some()
}

/// Start downloading model `id` in the background. Progress arrives as events.
pub fn start(app: AppHandle, id: &str) -> Result<(), String> {
    let spec = whimpr_models::builtin()
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("Unknown model {id}."))?;
    let cancel = try_begin(id)?;

    std::thread::spawn(move || {
        let emit = |dto: ProgressDto| {
            let _ = app.emit(PROGRESS_EVENT, dto);
        };
        let make = |state: &'static str, downloaded: u64, message: Option<String>| ProgressDto {
            id: spec.id.clone(),
            state,
            downloaded,
            total: spec.size_bytes,
            message,
        };
        let mut throttle = Throttle::new(Duration::from_millis(150));
        emit(make("downloading", 0, None));

        let dir = crate::hotkey::models_dir();
        let result = whimpr_models::download(&spec, &dir, &whimpr_models::Options::default(), &cancel, &mut |p| match p {
            Progress::Downloading { downloaded, .. } => {
                if throttle.ready(Instant::now(), false) {
                    emit(make("downloading", downloaded, None));
                }
            }
            Progress::Verifying => emit(make("verifying", spec.size_bytes, None)),
            Progress::Done => {}
        });

        match result {
            Ok(_) => {
                // Put the new model to work now, without a restart where the platform allows.
                let note = match crate::hotkey::reload_asr_model() {
                    Ok(true) => None,
                    Ok(false) => Some("Restart WhimprFlow to start using it.".to_string()),
                    Err(e) => Some(format!("It downloaded, but couldn't be loaded: {e}")),
                };
                finish();
                emit(make("done", spec.size_bytes, note));
            }
            Err(whimpr_models::DownloadError::Cancelled) => {
                finish();
                emit(make("cancelled", 0, None));
            }
            Err(e) => {
                finish();
                emit(make("error", 0, Some(e.to_string())));
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, file: &str, size: u64) -> ModelSpec {
        let mut s = whimpr_models::builtin().remove(0);
        s.id = id.into();
        s.file_name = file.into();
        s.size_bytes = size;
        s
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("whimpr-app-models-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_model_counts_as_installed_only_at_the_right_size() {
        let d = dir("installed");
        std::fs::write(d.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(d.join("b.bin"), vec![0u8; 40]).unwrap(); // a truncated copy
        let specs = [spec("a", "a.bin", 100), spec("b", "b.bin", 100), spec("c", "c.bin", 100)];
        let rows = describe(&specs, &d, None, None);
        assert_eq!(rows.iter().map(|r| r.installed).collect::<Vec<_>>(), [true, false, false]);
    }

    #[test]
    fn the_model_in_use_and_the_one_downloading_are_marked() {
        let d = dir("marks");
        let specs = [spec("a", "a.bin", 100), spec("b", "b.bin", 100)];
        let rows = describe(&specs, &d, Some("b.bin"), Some("a"));
        assert_eq!((rows[0].active, rows[0].downloading), (false, true));
        assert_eq!((rows[1].active, rows[1].downloading), (true, false));
    }

    #[test]
    fn progress_events_are_limited_but_state_changes_always_get_through() {
        let mut t = Throttle::new(Duration::from_millis(150));
        let start = Instant::now();
        assert!(t.ready(start, false), "the first one passes");
        assert!(!t.ready(start + Duration::from_millis(50), false));
        assert!(!t.ready(start + Duration::from_millis(149), false));
        assert!(t.ready(start + Duration::from_millis(150), false));
        assert!(t.ready(start + Duration::from_millis(151), true), "an important event never waits");
        assert!(!t.ready(start + Duration::from_millis(160), false), "and it restarts the interval");
    }

    #[test]
    fn only_one_download_runs_at_a_time_and_cancel_reaches_it() {
        // The slot is process-wide, so this is one test rather than several racing ones.
        assert_eq!(downloading_id(), None);
        assert!(!cancel(), "nothing to cancel yet");

        let flag = try_begin("whisper-base-en").expect("the first claim succeeds");
        assert_eq!(downloading_id().as_deref(), Some("whisper-base-en"));
        let refused = try_begin("whisper-large-v3-turbo").expect_err("a second one is refused");
        assert!(refused.contains("whisper-base-en"), "{refused}");

        assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));
        assert!(cancel());
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst), "the running download sees the cancel");

        finish();
        assert_eq!(downloading_id(), None);
        assert!(try_begin("whisper-large-v3-turbo").is_ok(), "the slot is free again");
        finish();
    }
}
