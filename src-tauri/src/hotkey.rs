//! Hold-Fn → pill wiring for the demo shell.
//!
//! This installs an in-process CoreGraphics event tap that feeds Fn key-down /
//! key-up into the real [`whimpr_core`] dictation state machine, and turns the
//! machine's actions into `whimpr://flowbar/state` events the overlay pill
//! renders. There is no audio or ASR yet, so a finalized session is simulated as
//! completing shortly after key release — enough to see the full
//! recording → transcribing → done → idle loop driven by the actual state machine.
//!
//! In the shipping product this hook lives in a separate sidecar process (so heavy
//! inference can't stall it); running it in-process is an acceptable macOS-only
//! path for this demo and the early milestones.

/// Dictionary entry shape sent to the Hub UI (auto-learned entries flagged).
#[derive(Clone, serde::Serialize)]
pub struct DictEntryDto {
    pub correct: String,
    pub mishears: Vec<String>,
    pub auto: bool,
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::c_void;
    use std::path::PathBuf;
    use super::DictEntryDto;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use serde::Serialize;
    use tauri::{AppHandle, Emitter};
    use whimpr_core::state::{Action, BarState};
    use whimpr_core::{
        AsrEngine, CleanupContext, CleanupMode, CleanupProvider, Input, PipelineEvent, StateMachine,
        TriggerToken,
    };
    use whimpr_ipc::BindingId;

    const OVERLAY_LABEL: &str = "whimpr_bar";

    /// A poisoned `Mutex` still holds valid data — the panic that poisoned it
    /// happened *after* some earlier mutation, not during this one, so the
    /// data itself isn't corrupt. Recovering it here means one panic on a
    /// background thread (capture callback, ASR decode, a cleanup HTTP call)
    /// degrades that one operation instead of permanently poisoning every
    /// future `.lock()` on the same static and bricking dictation until the
    /// app is relaunched.
    trait PoisonRecover<T> {
        fn lock_recover(&self) -> std::sync::MutexGuard<'_, T>;
    }
    impl<T> PoisonRecover<T> for Mutex<T> {
        fn lock_recover(&self) -> std::sync::MutexGuard<'_, T> {
            self.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    // --- CoreGraphics / CoreFoundation FFI (listen-only Fn tap) -----------
    type CFMachPortRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;
    type CFStringRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *mut c_void;
    type CGEventTapCallBack =
        extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventGetFlags(event: CGEventRef) -> u64;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFMachPortCreateRunLoopSource(
            allocator: CFAllocatorRef,
            port: CFMachPortRef,
            order: isize,
        ) -> CFRunLoopSourceRef;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFRunLoopRun();
        static kCFRunLoopDefaultMode: CFStringRef;
    }

    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    const K_CG_HEAD_INSERT: u32 = 0;
    const K_CG_TAP_OPTION_LISTEN_ONLY: u32 = 1;
    const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
    const EVENTS_OF_INTEREST: u64 = 1 << K_CG_EVENT_FLAGS_CHANGED;
    const FLAG_SECONDARY_FN: u64 = 0x0080_0000;
    const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
    const KEYCODE_FN: i64 = 63;
    const K_CG_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
    const K_CG_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static MACHINE: OnceLock<Mutex<StateMachine>> = OnceLock::new();
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    static FN_IS_DOWN: AtomicBool = AtomicBool::new(false);
    static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    /// Set once at startup if no Whisper model file exists on disk at all —
    /// distinct from "still loading", so the finalize path only shows the
    /// user a loud "no speech model" error for the real case, not a race
    /// against the ~1s background load right after launch.
    static ASR_MODEL_MISSING: AtomicBool = AtomicBool::new(false);
    /// Bundle id of the app that was frontmost at record-start = the paste target.
    /// Cleanup uses it to format for the medium (email vs. text vs. chat).
    static TARGET_APP: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    static CAPTURE: OnceLock<Mutex<Option<whimpr_audio::CaptureHandle>>> = OnceLock::new();
    static ASR: OnceLock<Arc<whimpr_asr::WhisperEngine>> = OnceLock::new();
    static OPENAI: OnceLock<Mutex<Option<whimpr_cleanup::OpenAiProvider>>> = OnceLock::new();
    static ANTHROPIC: OnceLock<Mutex<Option<whimpr_cleanup::AnthropicProvider>>> = OnceLock::new();
    static LOCAL: OnceLock<Mutex<Option<crate::local_llm::LocalWorker>>> = OnceLock::new();
    static SETTINGS: OnceLock<Mutex<whimpr_core::Settings>> = OnceLock::new();
    static DICTIONARY: OnceLock<Mutex<whimpr_core::DictionaryStore>> = OnceLock::new();
    static BRAIN: OnceLock<Mutex<whimpr_core::brain::Brain>> = OnceLock::new();
    static STATS: OnceLock<Mutex<whimpr_core::StatsStore>> = OnceLock::new();
    static STYLE: OnceLock<Mutex<whimpr_core::StyleStore>> = OnceLock::new();

    #[derive(Clone, Serialize)]
    struct BarPayload {
        state: &'static str,
    }

    #[derive(Clone, Serialize)]
    struct WavePayload {
        bars: Vec<f32>,
    }

    #[derive(Clone, Serialize)]
    struct TranscriptPayload {
        text: String,
    }

    /// Guards against a partial-transcribe tick overlapping a still-running one
    /// (Whisper re-decodes the whole buffer so far each tick; on a long
    /// utterance a decode can take longer than the tick interval).
    static PARTIAL_BUSY: AtomicBool = AtomicBool::new(false);
    /// How often to re-decode the in-progress recording for the live partial
    /// transcript shown in the pill.
    const PARTIAL_TICK: Duration = Duration::from_millis(900);
    /// Below this much audio, skip the decode — not enough signal yet to be
    /// worth a full Whisper pass.
    const PARTIAL_MIN_SECS: f32 = 0.6;

    /// The whisper ASR model to load: prefer the most accurate one present, in
    /// descending quality order, falling back to the small base model. Bigger
    /// English models mis-hear names/technical terms far less (and better ASR means
    /// less for cleanup and the dictionary to fix downstream).
    fn model_path() -> PathBuf {
        let dir = support_dir().join("models");
        for name in [
            "ggml-large-v3-turbo.bin",
            "ggml-medium.en.bin",
            "ggml-small.en.bin",
            "ggml-base.en.bin",
        ] {
            let p = dir.join(name);
            if p.exists() {
                return p;
            }
        }
        dir.join("ggml-base.en.bin")
    }

    fn support_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/WhimprFlow")
    }
    fn settings_path() -> PathBuf {
        support_dir().join("settings.json")
    }
    fn dict_path() -> PathBuf {
        support_dir().join("dictionary.json")
    }
    fn brain_path() -> PathBuf {
        support_dir().join("brain.json")
    }
    fn stats_path() -> PathBuf {
        support_dir().join("stats.json")
    }
    fn style_path() -> PathBuf {
        support_dir().join("style.json")
    }
    pub(crate) fn credentials_path() -> PathBuf {
        support_dir().join("credentials.json")
    }

    /// Seconds since the Unix epoch (UTC), or 0 if the clock is before the epoch.
    fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Log one completed dictation to the stats store (words, speaking time, text,
    /// target app) and persist it. Powers both the Hub stats and the history list.
    pub fn record_dictation(text: &str, duration_secs: f32) {
        let words = whimpr_core::stats::count_words(text);
        if words == 0 {
            return;
        }
        let app = TARGET_APP.get().and_then(|m| m.lock_recover().clone());
        if let Some(m) = STATS.get() {
            let mut store = m.lock_recover();
            let duration_ms = (duration_secs.max(0.0) * 1000.0) as u32;
            let chars = text.chars().count() as u32;
            store.record(words, duration_ms, chars, unix_now(), text.to_string(), app);
            let _ = store.save(&stats_path());
        }
    }

    /// The most recent dictations for the Hub Home history list.
    pub fn history(limit: usize) -> Vec<whimpr_core::HistoryItem> {
        STATS
            .get()
            .map(|m| m.lock_recover().history(limit))
            .unwrap_or_default()
    }

    /// The dictionary entries for the Hub Dictionary screen (auto-learned flagged).
    pub fn dictionary_entries() -> Vec<DictEntryDto> {
        DICTIONARY
            .get()
            .map(|m| {
                m.lock_recover()
                    .entries
                    .iter()
                    .map(|e| DictEntryDto {
                        correct: e.correct.clone(),
                        mishears: e.mishears.clone(),
                        auto: matches!(e.source, whimpr_core::DictSource::Auto),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Add a manual dictionary entry and persist.
    pub fn dictionary_add(correct: String, mishears: Vec<String>) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock_recover();
            store.add(correct, mishears, whimpr_core::DictSource::Manual);
            let _ = store.save(&dict_path());
        }
    }

    /// Remove a dictionary entry by spelling and persist.
    pub fn dictionary_remove(correct: &str) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock_recover();
            if store.remove(correct) {
                let _ = store.save(&dict_path());
            }
        }
    }

    /// Add an AUTO-learned entry (from the post-paste correction observer) and persist.
    /// Marked ✨ auto in the UI. No-op if it would duplicate an existing entry's data.
    pub fn dictionary_learn(correct: String, mishears: Vec<String>) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock_recover();
            store.add(correct, mishears, whimpr_core::DictSource::Auto);
            let _ = store.save(&dict_path());
        }
    }

    /// Teach the brain from (meant, heard) pairs, e.g. a calibration session or a
    /// correction, and save it.
    pub fn brain_learn(pairs: Vec<(String, String)>) {
        if let Some(m) = BRAIN.get() {
            let mut brain = m.lock_recover();
            for (meant, heard) in pairs {
                brain.observe(&meant, &heard);
            }
            let _ = brain.save(&brain_path());
        }
    }

    /// What the brain has noticed, for the Hub's Calibrate page.
    pub fn brain_summary() -> whimpr_core::brain::Summary {
        BRAIN
            .get()
            .map(|m| m.lock_recover().summary())
            .unwrap_or_else(|| whimpr_core::brain::Brain::default().summary())
    }

    /// Forget everything the brain learned.
    pub fn brain_reset() {
        if let Some(m) = BRAIN.get() {
            let mut brain = m.lock_recover();
            brain.reset();
            let _ = brain.save(&brain_path());
        }
    }

    /// Fix known mishears in a raw transcript: the dictionary's "also heard as"
    /// entries, then word pairs the brain has seen repeat.
    fn fix_mishears(raw: &str) -> String {
        let fixed = DICTIONARY
            .get()
            .map(|m| m.lock_recover().apply_replacements(raw))
            .unwrap_or_else(|| raw.to_string());
        BRAIN.get().map(|m| m.lock_recover().correct(&fixed)).unwrap_or(fixed)
    }

    /// The user's spellings (dictionary + words the brain has seen misheard) as a
    /// whisper `initial_prompt`.
    fn whisper_prompt() -> Option<String> {
        let mut words = DICTIONARY.get().map(|m| m.lock_recover().bias_words()).unwrap_or_default();
        if let Some(m) = BRAIN.get() {
            words.extend(m.lock_recover().bias_words(40));
        }
        whimpr_asr::vocab_prompt(&words)
    }

    static CAL_CAPTURE: OnceLock<Mutex<Option<whimpr_audio::CaptureHandle>>> = OnceLock::new();

    /// Start recording one calibration line (separate from the dictation hotkey).
    pub fn calibration_record_start() -> Result<(), String> {
        let slot = CAL_CAPTURE.get_or_init(|| Mutex::new(None));
        if slot.lock_recover().is_some() {
            return Ok(());
        }
        let handle = whimpr_audio::start(|_| {}).map_err(|e| format!("Couldn't start the microphone: {e}"))?;
        *slot.lock_recover() = Some(handle);
        Ok(())
    }

    /// Stop recording and return exactly what the recogniser heard: no
    /// replacements or cleanup, so it can be compared with the prompt text.
    pub fn calibration_record_stop() -> Result<String, String> {
        let handle = CAL_CAPTURE
            .get()
            .and_then(|s| s.lock_recover().take())
            .ok_or_else(|| "Nothing is being recorded.".to_string())?;
        let res = handle.stop().ok_or_else(|| "No audio was captured.".to_string())?;
        let asr = ASR.get().cloned().ok_or_else(|| "The speech model isn't ready yet.".to_string())?;
        let pcm = whimpr_audio::resample_to_16k(&res.samples, res.sample_rate);
        let t = asr
            .transcribe_with_prompt(&pcm, whisper_prompt().as_deref())
            .map_err(|e| e.to_string())?;
        Ok(t.text)
    }

    /// The learned style profile for one mode, for the Hub's Style page.
    pub fn style_summary(mode: whimpr_core::StyleMode) -> whimpr_core::StyleSummary {
        STYLE
            .get()
            .map(|m| m.lock_recover().summary(mode))
            .unwrap_or_else(|| whimpr_core::StyleStore::default().summary(mode))
    }

    /// All three modes' summaries in one call, for the Hub's Style page.
    pub fn style_summaries() -> std::collections::HashMap<whimpr_core::StyleMode, whimpr_core::StyleSummary> {
        whimpr_core::StyleMode::ALL.into_iter().map(|mode| (mode, style_summary(mode))).collect()
    }

    /// Wipe one mode's learned style profile (the Style page's "Reset" button).
    pub fn reset_style(mode: whimpr_core::StyleMode) {
        if let Some(m) = STYLE.get() {
            let mut store = m.lock_recover();
            store.reset(mode);
            let _ = store.save(&style_path());
        }
    }

    /// Aggregated stats for the Hub. `tz_offset_minutes` is the UI's
    /// `Date.getTimezoneOffset()` so day math matches the user's local clock.
    pub fn stats_summary(tz_offset_minutes: i32) -> whimpr_core::StatsSummary {
        STATS
            .get()
            .map(|m| m.lock_recover().summary(tz_offset_minutes, unix_now()))
            .unwrap_or_else(|| {
                whimpr_core::StatsStore::default().summary(tz_offset_minutes, unix_now())
            })
    }

    /// Read an API key from an env var or the local credentials file (see
    /// `whimpr_core::credentials` for why this isn't the OS keychain).
    fn read_key(account: &str, env_var: &str) -> Option<String> {
        if let Ok(k) = std::env::var(env_var) {
            let k = k.trim().to_string();
            if !k.is_empty() {
                return Some(k);
            }
        }
        whimpr_core::CredentialStore::load(&credentials_path()).get(account)
    }
    fn read_openai_key() -> Option<String> {
        read_key("openai_api_key", "OPENAI_API_KEY")
    }
    fn read_anthropic_key() -> Option<String> {
        read_key("anthropic_api_key", "ANTHROPIC_API_KEY")
    }

    /// A snapshot of the current settings.
    pub fn current_settings() -> whimpr_core::Settings {
        SETTINGS
            .get()
            .map(|m| m.lock_recover().clone())
            .unwrap_or_default()
    }
    /// Apply new settings and rebuild the cloud providers (picks up model changes).
    pub fn update_settings(new: whimpr_core::Settings) {
        if let Some(m) = SETTINGS.get() {
            *m.lock_recover() = new.clone();
        }
        let _ = new.save(&settings_path());
        rebuild_providers();
    }

    /// (Re)build the cloud cleanup providers from the current keys + settings. Called
    /// at startup and whenever a key or model changes, so edits take effect live.
    pub fn rebuild_providers() {
        let settings = current_settings();
        let openai = read_openai_key().map(|k| {
            whimpr_cleanup::OpenAiProvider::with_base_url(
                k,
                settings.openai_model.clone(),
                Some(settings.openai_base_url.clone()),
            )
        });
        let anthropic = read_anthropic_key()
            .map(|k| whimpr_cleanup::AnthropicProvider::new(k, settings.anthropic_model.clone()));
        eprintln!(
            "[whimpr] cleanup providers: openai={}, anthropic={}",
            openai.is_some(),
            anthropic.is_some()
        );
        match OPENAI.get() {
            Some(m) => *m.lock_recover() = openai,
            None => {
                let _ = OPENAI.set(Mutex::new(openai));
            }
        }
        match ANTHROPIC.get() {
            Some(m) => *m.lock_recover() = anthropic,
            None => {
                let _ = ANTHROPIC.set(Mutex::new(anthropic));
            }
        }
    }

    /// Clean a raw transcript per the current settings (mode + level), feeding in the
    /// dictionary vocabulary relevant to this utterance. Falls back to raw whenever
    /// cleanup is off, the provider is unavailable, it errors, or the gates reject it.
    fn clean_transcript(app: &AppHandle, raw: &str) -> String {
        let cleaned = clean_with_model(app, raw);
        review_transcript(raw, cleaned)
    }

    /// The last read: does the message make sense? Repairs only words that don't, using
    /// what was heard. Cloud models only; any failure keeps the message as it is.
    fn review_transcript(heard: &str, draft: String) -> String {
        let settings = current_settings();
        if !settings.review_enabled
            || matches!(settings.cleanup_mode, CleanupMode::Raw)
            || settings.cleanup_level.bypasses_llm()
        {
            return draft;
        }
        let vocab = DICTIONARY
            .get()
            .map(|d| d.lock_recover().prefilter(heard, 15))
            .unwrap_or_default();
        let outcome = match settings.cleanup_mode {
            CleanupMode::OpenAi => OPENAI
                .get()
                .and_then(|m| m.lock_recover().as_ref().map(|p| whimpr_core::review::run(p, heard, &draft, None, &vocab))),
            CleanupMode::Anthropic => ANTHROPIC
                .get()
                .and_then(|m| m.lock_recover().as_ref().map(|p| whimpr_core::review::run(p, heard, &draft, None, &vocab))),
            _ => None,
        };
        match outcome {
            Some(o) if o.changed => {
                let fixes: Vec<String> = o.changes.iter().map(|c| format!("{} → {}", c.from, c.to)).collect();
                eprintln!("[whimpr] SENSE CHECK fixed: {}", fixes.join(", "));
                o.text
            }
            Some(o) => {
                if let Some(why) = o.rejected {
                    eprintln!("[whimpr] sense check: {why}");
                }
                draft
            }
            None => draft,
        }
    }

    fn clean_with_model(app: &AppHandle, raw: &str) -> String {
        let settings = current_settings();
        let level = settings.cleanup_level;
        if matches!(settings.cleanup_mode, CleanupMode::Raw) || level.bypasses_llm() {
            return raw.to_string();
        }
        // Turn explicit spoken layout cues ("new line", "new paragraph") into break
        // markers up front — the model passes an opaque marker through reliably but
        // mangles the literal cue words. The model sees `raw` (with markers); the gate
        // and any raw fallback use `raw_out` (markers restored to real breaks) so we
        // never paste a "[[NL]]" token or lose an explicit break.
        let raw_norm = whimpr_core::cleanup::pre_normalize_layout(raw);
        let raw = raw_norm.as_str();
        let raw_out = whimpr_core::cleanup::post_process(&raw_norm);
        let vocab = DICTIONARY
            .get()
            .map(|d| d.lock_recover().prefilter(raw, 15))
            .unwrap_or_default();
        let app_bundle_id = TARGET_APP.get().and_then(|m| m.lock_recover().clone());
        if let Some(app) = app_bundle_id.as_deref() {
            eprintln!("[whimpr] cleanup target app: {app}");
        }
        let style_mode = whimpr_core::resolve_style_mode(settings.format_mode, app_bundle_id.as_deref());
        let style_summary = if settings.style_learning_enabled {
            STYLE.get().and_then(|m| m.lock_recover().summary_for_prompt(style_mode))
        } else {
            None
        };
        let ctx = CleanupContext {
            level,
            vocab,
            app_bundle_id,
            style_summary,
            format_mode: settings.format_mode,
            ..Default::default()
        };
        // Run the on-device model with the same prompt + per-app formatting.
        let run_local = || -> Option<anyhow::Result<String>> {
            LOCAL.get().and_then(|m| {
                m.lock_recover().as_mut().map(|w| {
                    // System prompt + few-shot demonstration turns + the transcript,
                    // so the on-device model actually produces newlines/lists and
                    // resolves self-corrections instead of just being told to.
                    let messages = whimpr_core::cleanup::build_messages(raw, &ctx);
                    w.cleanup(&messages)
                })
            })
        };
        // Selected provider, falling back to local when a cloud key can't be read
        // (so cleanup still runs) — and Local mode uses the worker directly.
        let result: Option<anyhow::Result<String>> = match settings.cleanup_mode {
            CleanupMode::OpenAi => OPENAI
                .get()
                .and_then(|m| m.lock_recover().as_ref().map(|p| p.cleanup(raw, &ctx)))
                .or_else(run_local),
            CleanupMode::Anthropic => ANTHROPIC
                .get()
                .and_then(|m| m.lock_recover().as_ref().map(|p| p.cleanup(raw, &ctx)))
                .or_else(run_local),
            CleanupMode::Local => run_local(),
            CleanupMode::Raw => None,
        };
        match result {
            Some(Ok(cleaned)) => {
                // Deterministic safety net: convert any leftover spoken layout cue the
                // model missed into real line breaks, strip stray code fences, cap blank
                // lines. Guarantees no "new line"/"new paragraph" word reaches the cursor.
                let cleaned = whimpr_core::cleanup::post_process(&cleaned);
                if whimpr_core::cleanup::evaluate_gates(&raw_out, &cleaned, level).passed() {
                    cleaned
                } else {
                    eprintln!("[whimpr] cleanup gate rejected the edit — pasting raw");
                    raw_out
                }
            }
            Some(Err(e)) => {
                eprintln!("[whimpr] cleanup failed ({e}) — pasting raw");
                raw_out
            }
            None => {
                if matches!(settings.cleanup_mode, CleanupMode::Local) {
                    crate::diag::notice(
                        app,
                        "Cleanup skipped — pasted raw",
                        "The on-device cleanup model isn't ready yet, so this dictation was \
                         pasted exactly as spoken, with no cleanup applied. It should be ready \
                         shortly; if this keeps happening, try switching Cleanup Mode to \
                         OpenAI or Anthropic in Settings.",
                    );
                } else {
                    crate::diag::notice(
                        app,
                        "Cleanup skipped — pasted raw",
                        "No API key is set for the selected cleanup provider, so this \
                         dictation was pasted exactly as spoken. Add a key under Settings → \
                         Cleanup Engine to enable cleanup.",
                    );
                }
                raw_out
            }
        }
    }

    /// While recording, periodically re-decode the audio captured so far with
    /// Whisper (local, no network/API cost) and emit it as a partial transcript
    /// so the pill can show a live "what you're saying" preview. Runs entirely
    /// off-thread; stops itself once `CAPTURE` is empty (stopped or discarded).
    /// The final cleaned transcript still comes from the one-shot pipeline in
    /// `StopCaptureAndFinalize` — this never touches the cloud cleanup LLM.
    fn spawn_partial_transcribe_loop(app: AppHandle) {
        std::thread::spawn(move || loop {
            std::thread::sleep(PARTIAL_TICK);
            let still_recording = CAPTURE
                .get()
                .map(|slot| slot.lock_recover().is_some())
                .unwrap_or(false);
            if !still_recording {
                // Clear any partial text left in the pill now that the final
                // (cleaned) transcript is about to take over.
                let _ = app.emit_to(
                    OVERLAY_LABEL,
                    "whimpr://transcript/partial",
                    TranscriptPayload { text: String::new() },
                );
                return;
            }
            if PARTIAL_BUSY.swap(true, Ordering::SeqCst) {
                continue; // previous decode still running; skip this tick
            }
            let Some(asr) = ASR.get().cloned() else {
                PARTIAL_BUSY.store(false, Ordering::SeqCst);
                continue;
            };
            let snapshot = CAPTURE
                .get()
                .and_then(|slot| slot.lock_recover().as_ref().map(|h| h.snapshot()));
            let Some((samples, sample_rate)) = snapshot else {
                PARTIAL_BUSY.store(false, Ordering::SeqCst);
                continue;
            };
            let duration_secs = if sample_rate == 0 {
                0.0
            } else {
                samples.len() as f32 / sample_rate as f32
            };
            if duration_secs < PARTIAL_MIN_SECS {
                PARTIAL_BUSY.store(false, Ordering::SeqCst);
                continue;
            }
            let app2 = app.clone();
            std::thread::spawn(move || {
                let pcm = whimpr_audio::resample_to_16k(&samples, sample_rate);
                if let Ok(t) = asr.transcribe(&pcm) {
                    let _ = app2.emit_to(
                        OVERLAY_LABEL,
                        "whimpr://transcript/partial",
                        TranscriptPayload { text: t.text },
                    );
                }
                PARTIAL_BUSY.store(false, Ordering::SeqCst);
            });
        });
    }

    /// Cancel the in-progress dictation (discard the audio, no paste) — the
    /// pill's ✕ button does this, equivalent to pressing Esc.
    pub fn cancel_dictation() {
        handle_input(Input::Trigger(TriggerToken::Cancel { at_ms: now_ms() }));
    }

    /// End the in-progress dictation as if the trigger key had been
    /// released — the pill's stop button does this, so a click finishes and
    /// pastes the same way releasing Fn would, without needing to keep the
    /// physical key held.
    pub fn stop_dictation() {
        handle_input(Input::Trigger(TriggerToken::Up {
            binding: BindingId::PushToTalk,
            at_ms: now_ms(),
        }));
    }

    fn now_ms() -> u64 {
        CLOCK.get().map(|c| c.elapsed().as_millis() as u64).unwrap_or(0)
    }

    fn bar_name(b: BarState) -> &'static str {
        match b {
            BarState::Idle => "idle",
            BarState::Recording => "recording",
            BarState::Locked => "locked",
            BarState::Transcribing => "transcribing",
            BarState::Done => "done",
            BarState::Cancelled => "cancelled",
            BarState::Error => "error",
        }
    }

    fn emit_bar(app: &AppHandle, state: &'static str) {
        eprintln!("[whimpr] pill -> {state}");
        let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarPayload { state });
    }

    /// Feed one input into the shared state machine and enact its actions.
    fn handle_input(input: Input) {
        let (Some(app), Some(machine)) = (APP.get(), MACHINE.get()) else {
            return;
        };
        let actions = {
            let mut m = machine.lock_recover();
            m.step(input)
        };
        for action in actions {
            apply_action(app, action);
        }
    }

    fn apply_action(app: &AppHandle, action: Action) {
        match action {
            Action::ShowBar(bar) => {
                emit_bar(app, bar_name(bar));
                // Let the "done" tick linger briefly before returning to idle.
                if bar == BarState::Done {
                    let app2 = app.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        emit_bar(&app2, "idle");
                    });
                }
            }
            // Start the microphone; stream real RMS bars to the pill waveform.
            // Runs off the tap thread so the mic-permission prompt can't stall keys.
            Action::StartCapture { .. } => {
                // Jump to whichever monitor the cursor is on before anything
                // else, so the pill shows up where the user is about to look
                // rather than wherever it was left on a different screen.
                crate::reposition_to_active_screen(app);
                let app_thread = app.clone();
                std::thread::spawn(move || {
                    let app_cb = app_thread.clone();
                    match whimpr_audio::start(move |bars| {
                        let _ = app_cb.emit_to(
                            OVERLAY_LABEL,
                            "whimpr://audio/waveform",
                            WavePayload { bars: bars.to_vec() },
                        );
                    }) {
                        Ok(handle) => {
                            *CAPTURE.get_or_init(|| Mutex::new(None)).lock_recover() = Some(handle);
                            spawn_partial_transcribe_loop(app_thread);
                        }
                        Err(e) => eprintln!("[whimpr] mic capture failed to start: {e}"),
                    }
                });
            }
            // Stop the mic, transcribe the buffered audio, and advance the machine.
            Action::StopCaptureAndFinalize { session } => {
                let app2 = app.clone();
                let handle = CAPTURE.get().and_then(|slot| slot.lock_recover().take());
                std::thread::spawn(move || {
                    // Whatever happens, return the pill to idle (done -> idle).
                    let finish =
                        || handle_input(Input::Pipeline(PipelineEvent::Committed { session }));
                    let Some(res) = handle.and_then(|h| h.stop()) else {
                        eprintln!("[whimpr] no audio captured");
                        finish();
                        return;
                    };
                    let peak = res.samples.iter().fold(0f32, |m, &s| m.max(s.abs()));
                    eprintln!(
                        "[whimpr] captured {} samples @ {} Hz (~{:.2}s), peak {:.4}",
                        res.samples.len(),
                        res.sample_rate,
                        res.duration_secs(),
                        peak
                    );
                    // Below ~600ms is very likely an accidental brief tap (the SPEC's
                    // own single-tap-no-op gate), so don't alarm the user over it —
                    // only surface a loud diagnostic for holds long enough to be a
                    // real, intentional dictation attempt.
                    let was_real_attempt = res.duration_secs() >= 0.6;
                    if peak < 0.005 {
                        eprintln!(
                            "[whimpr] ⚠ audio is silent — the mic isn't being captured. Grant \
                             Microphone access to your terminal (System Settings → Privacy & \
                             Security → Microphone), then fully quit + reopen it and rerun."
                        );
                        if was_real_attempt {
                            crate::diag::report(
                                &app2,
                                whimpr_core::InjectionFailure::NoAudioCaptured,
                            );
                        }
                        finish();
                        return;
                    }
                    let Some(asr) = ASR.get().cloned() else {
                        eprintln!("[whimpr] ASR not ready (model still loading or missing)");
                        if was_real_attempt && ASR_MODEL_MISSING.load(Ordering::SeqCst) {
                            crate::diag::report(&app2, whimpr_core::InjectionFailure::AsrUnavailable);
                        }
                        finish();
                        return;
                    };
                    let pcm = whimpr_audio::resample_to_16k(&res.samples, res.sample_rate);
                    match asr.transcribe_with_prompt(&pcm, whisper_prompt().as_deref()) {
                        Ok(t) => {
                            // Known mishears (voice calibration + dictionary) are fixed
                            // before anything else, so every cleanup mode benefits.
                            let raw = fix_mishears(&t.text);
                            eprintln!("[whimpr] TRANSCRIPT: \"{}\"", raw);
                            // Passively learn the user's voice from the raw transcript —
                            // pure local text counting, no LLM call, regardless of which
                            // cleanup mode is selected (even Raw/off) since this is about
                            // what they said, not how it gets cleaned up. Learned into the
                            // same mode bucket this dictation's cleanup will use.
                            let settings_now = current_settings();
                            if settings_now.style_learning_enabled {
                                if let Some(m) = STYLE.get() {
                                    let app_bundle_id = TARGET_APP.get().and_then(|m| m.lock_recover().clone());
                                    let style_mode = whimpr_core::resolve_style_mode(
                                        settings_now.format_mode,
                                        app_bundle_id.as_deref(),
                                    );
                                    let mut store = m.lock_recover();
                                    store.observe(style_mode, &raw);
                                    let _ = store.save(&style_path());
                                }
                            }
                            // Clean the transcript (cloud LLM if configured), then paste.
                            let text = clean_transcript(&app2, &raw);
                            if text != raw {
                                eprintln!("[whimpr] CLEANED:   \"{}\"", text);
                            }
                            if !text.is_empty() {
                                if let Err(e) = crate::paste::paste_text(&text) {
                                    eprintln!("[whimpr] paste failed: {e}");
                                    // Distinguish the two real causes: Accessibility was
                                    // never (or no longer) granted, vs. everything else
                                    // (clipboard contention, etc.) — `is_trusted()` is the
                                    // authoritative check, cheaper and more precise than
                                    // matching on the error string.
                                    let failure = if !crate::paste::is_trusted() {
                                        whimpr_core::InjectionFailure::AccessibilityNotGranted
                                    } else {
                                        whimpr_core::InjectionFailure::ClipboardUnavailable
                                    };
                                    crate::diag::report(&app2, failure);
                                } else {
                                    crate::diag::clear_last_error();
                                }
                                // Log words + speaking time for the Hub stats (WPM, streak…).
                                record_dictation(&text, res.duration_secs());
                                // Watch the field for a post-paste correction to learn (✨).
                                crate::autolearn::watch_correction(&text);
                            } else if was_real_attempt {
                                crate::diag::report(&app2, whimpr_core::InjectionFailure::EmptyTranscript);
                            }
                            let _ = app2.emit_to(
                                OVERLAY_LABEL,
                                "whimpr://transcript",
                                TranscriptPayload { text },
                            );
                        }
                        Err(e) => {
                            eprintln!("[whimpr] ASR error: {e}");
                            if was_real_attempt {
                                crate::diag::report(&app2, whimpr_core::InjectionFailure::AsrUnavailable);
                            }
                        }
                    }
                    finish();
                });
            }
            Action::DiscardCapture { .. } => {
                if let Some(slot) = CAPTURE.get() {
                    if let Some(handle) = slot.lock_recover().take() {
                        let _ = handle.stop();
                    }
                }
            }
            // The ASR path (StopCaptureAndFinalize) now drives pipeline completion.
            Action::RunPipeline { .. } => {}
            // PlayPing / WarnSessionCap: no-ops for now.
            _ => {}
        }
    }

    extern "C" fn tap_callback(
        _proxy: CGEventTapProxy,
        etype: u32,
        event: CGEventRef,
        _info: *mut c_void,
    ) -> CGEventRef {
        if etype == K_CG_TAP_DISABLED_BY_TIMEOUT || etype == K_CG_TAP_DISABLED_BY_USER_INPUT {
            let port = TAP_PORT.load(Ordering::SeqCst);
            if !port.is_null() {
                unsafe { CGEventTapEnable(port, true) };
            }
            return event;
        }
        if etype == K_CG_EVENT_FLAGS_CHANGED {
            let keycode =
                unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
            if keycode == KEYCODE_FN {
                let flags = unsafe { CGEventGetFlags(event) };
                let down = (flags & FLAG_SECONDARY_FN) != 0;
                let was_down = FN_IS_DOWN.swap(down, Ordering::SeqCst);
                let at_ms = now_ms();
                if down && !was_down {
                    eprintln!("[whimpr] Fn DOWN");
                    // Snapshot the paste target now, while the user's app is focused.
                    let target = crate::appctx::frontmost_bundle_id();
                    *TARGET_APP.get_or_init(|| Mutex::new(None)).lock_recover() = target;
                    handle_input(Input::Trigger(TriggerToken::Down {
                        binding: BindingId::PushToTalk,
                        at_ms,
                    }));
                } else if !down && was_down {
                    eprintln!("[whimpr] Fn UP");
                    handle_input(Input::Trigger(TriggerToken::Up {
                        binding: BindingId::PushToTalk,
                        at_ms,
                    }));
                }
            }
        }
        event
    }

    pub fn install(app: AppHandle) {
        let _ = APP.set(app);
        let _ = MACHINE.set(Mutex::new(StateMachine::new()));
        let _ = CLOCK.set(Instant::now());

        // Load the speech-to-text model off the main thread (it takes ~1s).
        std::thread::spawn(|| {
            let path = model_path();
            if !path.exists() {
                eprintln!("[whimpr] ASR model not found at {}", path.display());
                ASR_MODEL_MISSING.store(true, Ordering::SeqCst);
                return;
            }
            match whimpr_asr::WhisperEngine::load(&path) {
                Ok(engine) => {
                    let _ = ASR.set(Arc::new(engine));
                    eprintln!("[whimpr] ASR model loaded — ready to transcribe");
                }
                Err(e) => {
                    eprintln!("[whimpr] ASR model load failed: {e}");
                    ASR_MODEL_MISSING.store(true, Ordering::SeqCst);
                }
            }
        });

        // Load settings + dictionary, and build cloud providers from stored keys.
        let settings = whimpr_core::Settings::load(&settings_path());
        let dict = whimpr_core::DictionaryStore::load(&dict_path());
        eprintln!(
            "[whimpr] cleanup mode: {:?}, level: {:?}",
            settings.cleanup_mode, settings.cleanup_level
        );
        let _ = SETTINGS.set(Mutex::new(settings));
        let _ = DICTIONARY.set(Mutex::new(dict));
        let _ = BRAIN.set(Mutex::new(whimpr_core::brain::Brain::load(&brain_path())));
        let _ = STATS.set(Mutex::new(whimpr_core::StatsStore::load(&stats_path())));
        let _ = STYLE.set(Mutex::new(whimpr_core::StyleStore::load(&style_path())));
        rebuild_providers();

        // Start the local cleanup worker in the background (model load takes a few
        // seconds; the first local cleanup waits for it, subsequent ones are fast).
        std::thread::spawn(|| {
            let worker = crate::local_llm::spawn_default();
            let _ = LOCAL.set(Mutex::new(worker));
        });

        // Accessibility is the ONE permission that makes the Fn CGEventTap global AND
        // lets us post the Cmd+V paste into other apps. Without it, a keyboard tap is
        // silently limited to frontmost-only — the exact bug. Prompt for it up front.
        if crate::paste::is_trusted() {
            eprintln!("[whimpr] Accessibility granted — Fn works in every app, paste enabled");
        } else {
            eprintln!(
                "[whimpr] ⚠ Accessibility NOT granted — Fn only works while WhimprFlow is \
                 frontmost and paste is disabled. Prompting; grant WhimprFlow under System \
                 Settings → Privacy & Security → Accessibility (no relaunch needed)."
            );
            crate::paste::prompt_accessibility();
        }
        // Input Monitoring is NOT the gate for a CGEventTap — kept only as diagnostics.
        eprintln!(
            "[whimpr] (info) Input Monitoring: {}",
            crate::paste::input_monitoring_granted()
        );

        // Periodic tick drives the double-tap timeout / session cap.
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(100));
            handle_input(Input::Tick { now_ms: now_ms() });
        });

        // The event tap runs on a thread with its own CFRunLoop. CRITICAL: create it
        // ONLY after the process is trusted for Accessibility. macOS fixes a keyboard
        // tap's privilege at CGEventTapCreate time — a tap born untrusted is
        // permanently frontmost-only and is NOT upgraded when the grant later arrives.
        // Polling here also means the Fn key starts working the moment the user grants
        // Accessibility, without a relaunch.
        std::thread::spawn(|| {
            while !crate::paste::is_trusted() {
                std::thread::sleep(Duration::from_millis(500));
            }
            eprintln!("[whimpr] Accessibility present — creating global Fn tap");
            // Bug fix: this used to try CGEventTapCreate exactly once and, if it
            // came back null despite Accessibility being granted (the real-world
            // stale-TCC-entry case this app's own comments already knew about),
            // give up FOREVER with only an eprintln — the Fn key would then do
            // nothing for the rest of the run, indistinguishable from "text isn't
            // typed" to the user, with zero visible explanation. Now: report it
            // loudly once, and keep retrying — toggling the Accessibility entry
            // off/on in System Settings, or removing and re-adding it, fixes the
            // stale grant without requiring a relaunch, and this way that fix is
            // picked up automatically.
            let mut reported = false;
            let port = loop {
                let port = unsafe {
                    CGEventTapCreate(
                        K_CG_SESSION_EVENT_TAP,
                        K_CG_HEAD_INSERT,
                        K_CG_TAP_OPTION_LISTEN_ONLY,
                        EVENTS_OF_INTEREST,
                        tap_callback,
                        null_mut(),
                    )
                };
                if !port.is_null() {
                    break port;
                }
                eprintln!(
                    "[whimpr] Fn tap null despite Accessibility — likely a stale TCC entry from \
                     an earlier build. Run: tccutil reset Accessibility com.whimpr.whimprflow, \
                     or toggle WhimprFlow off/on under System Settings → Privacy & Security → \
                     Accessibility. Retrying…"
                );
                if !reported {
                    if let Some(app) = APP.get() {
                        crate::diag::report(app, whimpr_core::InjectionFailure::HotkeyTapFailed);
                    }
                    reported = true;
                }
                std::thread::sleep(Duration::from_secs(5));
            };
            if reported {
                eprintln!("[whimpr] Fn tap recovered — the key is live now.");
                crate::diag::clear_last_error();
            }
            TAP_PORT.store(port, Ordering::SeqCst);
            unsafe {
                let source = CFMachPortCreateRunLoopSource(null(), port, 0);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode);
                CGEventTapEnable(port, true);
                CFRunLoopRun();
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    brain_learn, brain_reset, brain_summary, calibration_record_start, calibration_record_stop,
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, reset_style, stats_summary,
    stop_dictation, style_summaries, update_settings,
};

// Windows uses the real (but unverified) platform layer in `crate::win`.
#[cfg(target_os = "windows")]
pub use crate::win::{
    brain_learn, brain_reset, brain_summary, calibration_record_start, calibration_record_stop,
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, reset_style, stats_summary,
    stop_dictation, style_summaries, update_settings,
};

// Other platforms (Linux, etc.): inert stubs so the crate still builds.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod other {
    pub fn install(_app: tauri::AppHandle) {}
    pub fn current_settings() -> whimpr_core::Settings {
        whimpr_core::Settings::default()
    }
    pub fn update_settings(_new: whimpr_core::Settings) {}
    pub fn rebuild_providers() {}
    pub fn stats_summary(tz_offset_minutes: i32) -> whimpr_core::StatsSummary {
        whimpr_core::StatsStore::default().summary(tz_offset_minutes, 0)
    }
    pub fn history(_limit: usize) -> Vec<whimpr_core::HistoryItem> {
        Vec::new()
    }
    pub fn dictionary_entries() -> Vec<super::DictEntryDto> {
        Vec::new()
    }
    pub fn dictionary_add(_correct: String, _mishears: Vec<String>) {}
    pub fn dictionary_remove(_correct: &str) {}
    pub fn dictionary_learn(_correct: String, _mishears: Vec<String>) {}
    pub fn brain_learn(_pairs: Vec<(String, String)>) {}
    pub fn brain_summary() -> whimpr_core::brain::Summary {
        whimpr_core::brain::Brain::default().summary()
    }
    pub fn brain_reset() {}
    pub fn calibration_record_start() -> Result<(), String> {
        Err("Voice calibration recording isn't available on this platform yet.".into())
    }
    pub fn calibration_record_stop() -> Result<String, String> {
        Err("Voice calibration recording isn't available on this platform yet.".into())
    }
    pub fn cancel_dictation() {}
    pub fn stop_dictation() {}
    pub fn style_summary(mode: whimpr_core::StyleMode) -> whimpr_core::StyleSummary {
        whimpr_core::StyleStore::default().summary(mode)
    }
    pub fn style_summaries() -> std::collections::HashMap<whimpr_core::StyleMode, whimpr_core::StyleSummary> {
        whimpr_core::StyleMode::ALL.into_iter().map(|mode| (mode, style_summary(mode))).collect()
    }
    pub fn reset_style(_mode: whimpr_core::StyleMode) {}
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use other::{
    brain_learn, brain_reset, brain_summary, calibration_record_start, calibration_record_stop,
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, reset_style, stats_summary,
    stop_dictation, style_summaries, update_settings,
};
