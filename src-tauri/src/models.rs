//! Speech-model downloads for the Hub: what's installed, start/cancel a download, progress
//! events. The downloading itself is `whimpr-models`; this connects it to the app.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use whimpr_core::AsrEngineId;
use whimpr_models::{ModelSpec, Progress, PARAKEET_ID};

/// The event the Hub listens to.
const PROGRESS_EVENT: &str = "whimpr://models/progress";

/// One row of the models list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelDto {
    pub id: String,
    pub title: String,
    pub blurb: String,
    /// Download size of the whole model.
    pub size_bytes: u64,
    /// Every file is in the models folder at the right size.
    pub installed: bool,
    /// This is the model speech recognition is using.
    pub active: bool,
    /// Installed, not in use, and the person can switch to it now.
    pub selectable: bool,
    pub downloading: bool,
}

/// Which model speech recognition is using, given the chosen engine: Parakeet if it is chosen
/// and installed, otherwise the best whisper model present. `None` if nothing matches (no
/// model, or only a hand-placed one the registry doesn't list).
pub fn active_model_id(engine: AsrEngineId, specs: &[ModelSpec], dir: &Path) -> Option<String> {
    if engine == AsrEngineId::OnnxParakeet {
        if let Some(parakeet) = specs.iter().find(|s| s.id == PARAKEET_ID && s.is_installed(dir)) {
            return Some(parakeet.id.clone());
        }
    }
    let best = whimpr_models::best_whisper_model(dir)?;
    let file = best.file_name()?.to_string_lossy().into_owned();
    specs.iter().find(|s| s.id != PARAKEET_ID && s.files.first().is_some_and(|f| f.name == file)).map(|s| s.id.clone())
}

/// Describe `specs` for the UI. `active_id` is the model in use, `downloading` the id being
/// fetched right now.
pub fn describe(specs: &[ModelSpec], dir: &Path, active_id: Option<&str>, downloading: Option<&str>) -> Vec<ModelDto> {
    let parakeet_active = active_id == Some(PARAKEET_ID);
    specs
        .iter()
        .map(|s| {
            let installed = s.is_installed(dir);
            let active = active_id == Some(s.id.as_str());
            // Whisper models pick themselves (the best installed one), so the only switch is
            // between Parakeet and whisper: to Parakeet when it's installed, back to whisper
            // when Parakeet is in use.
            let selectable = installed && !active && (s.id == PARAKEET_ID || parakeet_active);
            ModelDto {
                id: s.id.clone(),
                title: s.title.clone(),
                blurb: s.blurb.clone(),
                size_bytes: s.total_bytes(),
                installed,
                active,
                selectable,
                downloading: downloading == Some(s.id.as_str()),
            }
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
    let specs = whimpr_models::builtin();
    let active = active_model_id(crate::hotkey::current_settings().asr_engine, &specs, &dir);
    describe(&specs, &dir, active.as_deref(), downloading_id().as_deref())
}

/// Choose which speech engine to use: Parakeet for the Parakeet model, whisper for any
/// whisper model. The engine is swapped in the background.
pub fn use_model(id: &str) -> Result<(), String> {
    let spec = whimpr_models::builtin().into_iter().find(|m| m.id == id).ok_or_else(|| format!("Unknown model {id}."))?;
    if !spec.is_installed(&crate::hotkey::models_dir()) {
        return Err(format!("{} isn't downloaded yet.", spec.title));
    }
    let mut settings = crate::hotkey::current_settings();
    settings.asr_engine = if id == PARAKEET_ID { AsrEngineId::OnnxParakeet } else { AsrEngineId::WhisperCpp };
    crate::hotkey::update_settings(settings);
    Ok(())
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
            total: spec.total_bytes(),
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
            Progress::Verifying => emit(make("verifying", spec.total_bytes(), None)),
            Progress::Done => {}
        });

        match result {
            Ok(()) => {
                // Put a new whisper model to work now, without a restart where the platform
                // allows. Parakeet is opt-in: downloading it doesn't switch to it.
                let wanted = crate::hotkey::current_settings().asr_engine;
                let in_use = spec.id != PARAKEET_ID || wanted == AsrEngineId::OnnxParakeet;
                let note = if in_use {
                    match crate::hotkey::reload_asr_model(wanted) {
                        Ok(true) => None,
                        Ok(false) => Some("Restart WhimprFlow to start using it.".to_string()),
                        Err(e) => Some(format!("It downloaded, but couldn't be loaded: {e}")),
                    }
                } else {
                    Some("Downloaded. Choose \"Use\" to switch to it.".to_string())
                };
                finish();
                emit(make("done", spec.total_bytes(), note));
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

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("whimpr-app-models-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Put a model's files in place at their real sizes (sparse, so 650 MB costs nothing).
    fn install(spec: &ModelSpec, models_dir: &Path) {
        let folder = spec.install_dir(models_dir);
        std::fs::create_dir_all(&folder).unwrap();
        for f in &spec.files {
            std::fs::File::create(folder.join(&f.name)).unwrap().set_len(f.size_bytes).unwrap();
        }
    }

    fn registry() -> (Vec<ModelSpec>, ModelSpec, ModelSpec, ModelSpec) {
        let specs = whimpr_models::builtin();
        let find = |id: &str| specs.iter().find(|s| s.id == id).unwrap().clone();
        let (fast, best, parakeet) = (find(whimpr_models::DEFAULT_ID), find("whisper-large-v3-turbo"), find(PARAKEET_ID));
        (specs, fast, best, parakeet)
    }

    fn row<'a>(rows: &'a [ModelDto], id: &str) -> &'a ModelDto {
        rows.iter().find(|r| r.id == id).unwrap()
    }

    #[test]
    fn a_model_counts_as_installed_only_when_every_file_is_complete() {
        let d = dir("installed");
        let (specs, fast, _, parakeet) = registry();
        install(&fast, &d);
        // Three of Parakeet's four files: not a model yet.
        let folder = parakeet.install_dir(&d);
        std::fs::create_dir_all(&folder).unwrap();
        for f in parakeet.files.iter().take(3) {
            std::fs::File::create(folder.join(&f.name)).unwrap().set_len(f.size_bytes).unwrap();
        }
        let rows = describe(&specs, &d, None, None);
        assert!(row(&rows, &fast.id).installed);
        assert!(!row(&rows, PARAKEET_ID).installed);
        assert_eq!(row(&rows, PARAKEET_ID).size_bytes, parakeet.total_bytes(), "the size shown is the whole download");
    }

    #[test]
    fn the_best_installed_whisper_model_is_the_one_in_use_by_default() {
        let d = dir("active-whisper");
        let (specs, fast, best, _) = registry();
        assert_eq!(active_model_id(AsrEngineId::WhisperCpp, &specs, &d), None, "nothing installed");
        install(&fast, &d);
        assert_eq!(active_model_id(AsrEngineId::WhisperCpp, &specs, &d).as_deref(), Some(fast.id.as_str()));
        install(&best, &d);
        assert_eq!(active_model_id(AsrEngineId::WhisperCpp, &specs, &d).as_deref(), Some(best.id.as_str()));
    }

    #[test]
    fn choosing_parakeet_without_it_installed_keeps_using_whisper() {
        let d = dir("parakeet-missing");
        let (specs, fast, _, _) = registry();
        install(&fast, &d);
        assert_eq!(active_model_id(AsrEngineId::OnnxParakeet, &specs, &d).as_deref(), Some(fast.id.as_str()));
    }

    #[test]
    fn parakeet_installed_can_be_switched_to_and_back() {
        let d = dir("switching");
        let (specs, fast, _, parakeet) = registry();
        install(&fast, &d);
        install(&parakeet, &d);

        // Whisper chosen: Parakeet is installed but not in use, and can be switched to.
        let active = active_model_id(AsrEngineId::WhisperCpp, &specs, &d);
        let rows = describe(&specs, &d, active.as_deref(), None);
        assert!(row(&rows, &fast.id).active && !row(&rows, &fast.id).selectable);
        assert!(!row(&rows, PARAKEET_ID).active && row(&rows, PARAKEET_ID).selectable);

        // Parakeet chosen: it is in use, and whisper can be switched back to.
        let active = active_model_id(AsrEngineId::OnnxParakeet, &specs, &d);
        assert_eq!(active.as_deref(), Some(PARAKEET_ID));
        let rows = describe(&specs, &d, active.as_deref(), None);
        assert!(row(&rows, PARAKEET_ID).active && !row(&rows, PARAKEET_ID).selectable);
        assert!(!row(&rows, &fast.id).active && row(&rows, &fast.id).selectable);
    }

    #[test]
    fn a_model_that_is_not_installed_is_never_selectable() {
        let d = dir("not-installed");
        let (specs, fast, _, _) = registry();
        install(&fast, &d);
        let rows = describe(&specs, &d, Some(&fast.id), None);
        assert!(rows.iter().filter(|r| !r.installed).all(|r| !r.selectable));
    }

    #[test]
    fn the_model_being_downloaded_is_marked() {
        let d = dir("marks");
        let (specs, _, best, _) = registry();
        let rows = describe(&specs, &d, None, Some(&best.id));
        assert!(row(&rows, &best.id).downloading);
        assert_eq!(rows.iter().filter(|r| r.downloading).count(), 1);
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
