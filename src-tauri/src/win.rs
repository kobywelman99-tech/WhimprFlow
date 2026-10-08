//! Windows platform layer for WhimprFlow: a low-level keyboard hook for
//! push-to-talk, clipboard+SendInput text injection, and foreground-app detection,
//! plus the same dictation pipeline (audio → Whisper ASR → cleanup LLM → paste) and
//! the Hub-facing settings/stats/dictionary functions the Tauri commands call.
//!
//! ⚠️ UNVERIFIED: this module was written on macOS and has **never been compiled or
//! run on Windows**. The shared crates (audio, ASR, cleanup, core) are
//! cross-platform, but this Win32 glue will almost certainly need fixes before it
//! builds and runs. It is `cfg(target_os = "windows")` so it does not affect — and
//! is not checked by — the macOS build. Treat it as a starting point, not a
//! shipping port. Default push-to-talk key: Right Ctrl.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::GetModuleBaseNameW;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL,
    VK_RCONTROL, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, SetWindowsHookExW,
    HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use whimpr_core::{AsrEngine, CleanupContext, CleanupMode, CleanupProvider, StatsSummary};

const OVERLAY_LABEL: &str = "whimpr_bar";
/// Push-to-talk key. Right Ctrl by default (Ctrl+Win chords land in a later pass).
const PTT_VK: u16 = VK_RCONTROL.0;

static APP: OnceLock<AppHandle> = OnceLock::new();
static CLOCK: OnceLock<Instant> = OnceLock::new();
static RECORDING: AtomicBool = AtomicBool::new(false);
/// Set once at startup if no Whisper model file exists on disk at all —
/// distinct from "still loading". Mirrors the macOS flag in `hotkey.rs`.
static ASR_MODEL_MISSING: AtomicBool = AtomicBool::new(false);
static CAPTURE: OnceLock<Mutex<Option<whimpr_audio::CaptureHandle>>> = OnceLock::new();
static ASR: OnceLock<Arc<whimpr_asr::WhisperEngine>> = OnceLock::new();
static LOCAL: OnceLock<Mutex<Option<crate::local_llm::LocalWorker>>> = OnceLock::new();
static OPENAI: OnceLock<Mutex<Option<whimpr_cleanup::OpenAiProvider>>> = OnceLock::new();
static SETTINGS: OnceLock<Mutex<whimpr_core::Settings>> = OnceLock::new();
static DICTIONARY: OnceLock<Mutex<whimpr_core::DictionaryStore>> = OnceLock::new();
static BRAIN: OnceLock<Mutex<whimpr_core::brain::Brain>> = OnceLock::new();
static STATS: OnceLock<Mutex<whimpr_core::StatsStore>> = OnceLock::new();
static STYLE: OnceLock<Mutex<whimpr_core::StyleStore>> = OnceLock::new();

fn support_dir() -> std::path::PathBuf {
    // %APPDATA%\WhimprFlow
    let base = std::env::var("APPDATA").unwrap_or_default();
    std::path::PathBuf::from(base).join("WhimprFlow")
}
fn settings_path() -> std::path::PathBuf {
    support_dir().join("settings.json")
}
fn brain_path() -> std::path::PathBuf {
    dict_path().with_file_name("brain.json")
}

fn dict_path() -> std::path::PathBuf {
    support_dir().join("dictionary.json")
}
fn stats_path() -> std::path::PathBuf {
    support_dir().join("stats.json")
}
fn style_path() -> std::path::PathBuf {
    support_dir().join("style.json")
}
fn credentials_path() -> std::path::PathBuf {
    support_dir().join("credentials.json")
}
fn whisper_model_path() -> std::path::PathBuf {
    let dir = support_dir().join("models");
    for name in ["ggml-medium.en.bin", "ggml-small.en.bin", "ggml-base.en.bin"] {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    dir.join("ggml-base.en.bin")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    CLOCK.get().map(|c| c.elapsed().as_millis() as u64).unwrap_or(0)
}

fn emit_bar(state: &'static str) {
    if let Some(app) = APP.get() {
        #[derive(Clone, serde::Serialize)]
        struct P {
            state: &'static str,
        }
        let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", P { state });
    }
}

/// The foreground process's executable name (e.g. "chrome.exe"), for per-app
/// cleanup formatting — the Windows analogue of the macOS bundle id.
fn foreground_app() -> Option<String> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle =
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let len = GetModuleBaseNameW(handle, None, &mut buf);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

// ── Text injection: clipboard + Ctrl+V via SendInput ────────────────────────────

fn key_event(vk: u16, up: bool) -> INPUT {
    let mut ki = KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk),
        ..Default::default()
    };
    if up {
        ki.dwFlags = KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 { ki },
    }
}

pub fn paste_text(text: &str) -> anyhow::Result<()> {
    use arboard::Clipboard;
    let mut cb = Clipboard::new()?;
    let saved = cb.get_text().ok();
    cb.set_text(text.to_string())?;
    std::thread::sleep(Duration::from_millis(60));
    let inputs = [
        key_event(VK_CONTROL.0, false),
        key_event(VK_V.0, false),
        key_event(VK_V.0, true),
        key_event(VK_CONTROL.0, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    std::thread::sleep(Duration::from_millis(150));
    if let Some(prev) = saved {
        let _ = cb.set_text(prev);
    }
    Ok(())
}

// ── Cleanup (shared, cross-platform building blocks) ────────────────────────────

fn current_settings_inner() -> whimpr_core::Settings {
    SETTINGS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

fn clean_transcript(raw: &str) -> String {
    let cleaned = clean_with_model(raw);
    review_transcript(raw, cleaned)
}

/// The last read: does the message make sense? Repairs only words that don't, using
/// what was heard. Cloud models only; any failure keeps the message as it is.
fn review_transcript(heard: &str, draft: String) -> String {
    let settings = current_settings_inner();
    if !settings.review_enabled
        || matches!(settings.cleanup_mode, CleanupMode::Raw)
        || settings.cleanup_level.bypasses_llm()
    {
        return draft;
    }
    let vocab = DICTIONARY
        .get()
        .map(|d| d.lock().unwrap().prefilter(heard, 15))
        .unwrap_or_default();
    let outcome = match settings.cleanup_mode {
        CleanupMode::OpenAi => OPENAI
            .get()
            .and_then(|m| m.lock().unwrap().as_ref().map(|p| whimpr_core::review::run(p, heard, &draft, None, &vocab))),
        CleanupMode::Anthropic => ANTHROPIC
            .get()
            .and_then(|m| m.lock().unwrap().as_ref().map(|p| whimpr_core::review::run(p, heard, &draft, None, &vocab))),
        _ => None,
    };
    match outcome {
        Some(o) if o.changed => o.text,
        _ => draft,
    }
}

fn clean_with_model(raw: &str) -> String {
    let settings = current_settings_inner();
    let level = settings.cleanup_level;
    if matches!(settings.cleanup_mode, CleanupMode::Raw) || level.bypasses_llm() {
        return raw.to_string();
    }
    let raw_norm = whimpr_core::cleanup::pre_normalize_layout(raw);
    let raw_out = whimpr_core::cleanup::post_process(&raw_norm);
    let vocab = DICTIONARY
        .get()
        .map(|d| d.lock().unwrap().prefilter(&raw_norm, 15))
        .unwrap_or_default();
    let app_bundle_id = foreground_app();
    let style_mode = whimpr_core::resolve_style_mode(settings.format_mode, app_bundle_id.as_deref());
    let style_summary = if settings.style_learning_enabled {
        STYLE.get().and_then(|m| m.lock().unwrap().summary_for_prompt(style_mode))
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
    let run_local = || -> Option<anyhow::Result<String>> {
        LOCAL.get().and_then(|m| {
            m.lock().unwrap().as_mut().map(|w| {
                let messages = whimpr_core::cleanup::build_messages(&raw_norm, &ctx);
                w.cleanup(&messages)
            })
        })
    };
    let result = match settings.cleanup_mode {
        CleanupMode::OpenAi => OPENAI
            .get()
            .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(&raw_norm, &ctx)))
            .or_else(run_local),
        CleanupMode::Local => run_local(),
        _ => run_local(),
    };
    match result {
        Some(Ok(cleaned)) => {
            let cleaned = whimpr_core::cleanup::post_process(&cleaned);
            if whimpr_core::cleanup::evaluate_gates(&raw_out, &cleaned, level).passed() {
                cleaned
            } else {
                raw_out
            }
        }
        _ => raw_out,
    }
}

fn record_dictation(text: &str, duration_secs: f32, app: Option<String>) {
    let words = whimpr_core::stats::count_words(text);
    if words == 0 {
        return;
    }
    if let Some(m) = STATS.get() {
        let mut store = m.lock().unwrap();
        let duration_ms = (duration_secs.max(0.0) * 1000.0) as u32;
        let chars = text.chars().count() as u32;
        store.record(words, duration_ms, chars, unix_now(), text.to_string(), app);
        let _ = store.save(&stats_path());
    }
}

// ── The push-to-talk pipeline ───────────────────────────────────────────────────

fn on_ptt_down() {
    if RECORDING.swap(true, Ordering::SeqCst) {
        return; // already recording
    }
    let _ = now_ms();
    emit_bar("recording");
    std::thread::spawn(|| match whimpr_audio::start(|_: &[f32]| {}) {
        Ok(handle) => {
            *CAPTURE.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(handle);
        }
        Err(e) => eprintln!("[whimpr:win] mic capture failed: {e}"),
    });
}

fn on_ptt_up() {
    if !RECORDING.swap(false, Ordering::SeqCst) {
        return; // wasn't recording
    }
    emit_bar("idle");
    let app = foreground_app();
    let handle = CAPTURE.get().and_then(|slot| slot.lock().unwrap().take());
    std::thread::spawn(move || {
        // Below ~600ms is very likely an accidental brief tap, so don't alarm
        // the user over it — only surface a loud diagnostic for holds long
        // enough to be a real, intentional dictation attempt. Bug fix: this
        // whole function used to fail silently on every one of these paths
        // (no eprintln, no UI signal, nothing) — indistinguishable from
        // "held the key, spoke, nothing happened".
        let Some(res) = handle.and_then(|h| h.stop()) else {
            eprintln!("[whimpr:win] no audio captured");
            return;
        };
        let was_real_attempt = res.duration_secs() >= 0.6;
        let peak = res.samples.iter().fold(0f32, |m, &s| m.max(s.abs()));
        if peak < 0.005 {
            eprintln!("[whimpr:win] ⚠ audio is silent — the mic isn't being captured");
            if was_real_attempt {
                if let Some(app) = APP.get() {
                    crate::diag::report(app, whimpr_core::InjectionFailure::NoAudioCaptured);
                }
            }
            return;
        }
        let Some(asr) = ASR.get().cloned() else {
            eprintln!("[whimpr:win] ASR not ready (model still loading or missing)");
            if was_real_attempt && ASR_MODEL_MISSING.load(Ordering::SeqCst) {
                if let Some(app) = APP.get() {
                    crate::diag::report(app, whimpr_core::InjectionFailure::AsrUnavailable);
                }
            }
            return;
        };
        let pcm = whimpr_audio::resample_to_16k(&res.samples, res.sample_rate);
        let mut words = DICTIONARY.get().map(|m| m.lock().unwrap().bias_words()).unwrap_or_default();
        if let Some(m) = BRAIN.get() {
            words.extend(m.lock().unwrap().bias_words(40));
        }
        let prompt = whimpr_asr::vocab_prompt(&words);
        match asr.transcribe_with_prompt(&pcm, prompt.as_deref()) {
            Ok(mut t) => {
                // Known mishears (voice calibration + dictionary) are fixed first.
                if let Some(m) = DICTIONARY.get() {
                    t.text = m.lock().unwrap().apply_replacements(&t.text);
                }
                if let Some(m) = BRAIN.get() {
                    t.text = m.lock().unwrap().correct(&t.text);
                }
                let settings_now = current_settings_inner();
                if settings_now.style_learning_enabled {
                    if let Some(m) = STYLE.get() {
                        let style_mode = whimpr_core::resolve_style_mode(
                            settings_now.format_mode,
                            foreground_app().as_deref(),
                        );
                        let mut store = m.lock().unwrap();
                        store.observe(style_mode, &t.text);
                        let _ = store.save(&style_path());
                    }
                }
                let text = clean_transcript(&t.text);
                if !text.is_empty() {
                    if let Err(e) = paste_text(&text) {
                        eprintln!("[whimpr:win] paste failed: {e}");
                        if let Some(app) = APP.get() {
                            crate::diag::report(app, whimpr_core::InjectionFailure::ClipboardUnavailable);
                        }
                    } else {
                        crate::diag::clear_last_error();
                    }
                    record_dictation(&text, res.duration_secs(), app);
                } else if was_real_attempt {
                    if let Some(app) = APP.get() {
                        crate::diag::report(app, whimpr_core::InjectionFailure::EmptyTranscript);
                    }
                }
            }
            Err(e) => {
                eprintln!("[whimpr:win] ASR error: {e}");
                if was_real_attempt {
                    if let Some(app) = APP.get() {
                        crate::diag::report(app, whimpr_core::InjectionFailure::AsrUnavailable);
                    }
                }
            }
        }
    });
}

/// End the in-progress dictation as if the PTT key had been released — the
/// pill's stop button does this. ⚠️ Same unverified status as the rest of
/// this file (see module doc comment).
pub fn stop_dictation() {
    on_ptt_up();
}

/// Cancel the in-progress dictation: stop capture and discard the audio
/// without transcribing or pasting — the pill's ✕ button does this.
pub fn cancel_dictation() {
    if !RECORDING.swap(false, Ordering::SeqCst) {
        return; // wasn't recording
    }
    emit_bar("cancelled");
    if let Some(slot) = CAPTURE.get() {
        if let Some(handle) = slot.lock().unwrap().take() {
            let _ = handle.stop();
        }
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(500));
        emit_bar("idle");
    });
}

// ── Low-level keyboard hook ─────────────────────────────────────────────────────

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode as u16;
        if vk == PTT_VK {
            match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => on_ptt_down(),
                WM_KEYUP | WM_SYSKEYUP => on_ptt_up(),
                _ => {}
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

/// Install the hook on a dedicated thread with its own message pump (required for
/// WH_KEYBOARD_LL to deliver events).
///
/// Bug fix: this used to try `SetWindowsHookExW` exactly once and, on failure,
/// give up FOREVER with only an `eprintln!` — Right Ctrl would then do nothing
/// for the rest of the run, indistinguishable from "text isn't typed" with no
/// visible explanation. Now it reports the failure loudly (once) and keeps
/// retrying, so recovering (e.g. after whatever was holding a conflicting
/// global hook closes) doesn't require relaunching WhimprFlow.
fn spawn_hook_thread() {
    std::thread::spawn(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let mut reported = false;
        let hook = loop {
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hinst, 0) {
                Ok(h) => break h,
                Err(_) => {
                    eprintln!("[whimpr:win] failed to install keyboard hook — retrying…");
                    if !reported {
                        if let Some(app) = APP.get() {
                            crate::diag::report(app, whimpr_core::InjectionFailure::HotkeyTapFailed);
                        }
                        reported = true;
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        };
        if reported {
            eprintln!("[whimpr:win] keyboard hook recovered — Right Ctrl is live now.");
            crate::diag::clear_last_error();
        }
        let _ = hook; // keeps the hook alive for the lifetime of this thread
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {}
    });
}

// ── Public surface (mirrors the macOS `hotkey::` functions the commands call) ────

pub fn install(app: AppHandle) {
    let _ = APP.set(app);
    let _ = CLOCK.set(Instant::now());
    let _ = SETTINGS.set(Mutex::new(whimpr_core::Settings::load(&settings_path())));
    let _ = DICTIONARY.set(Mutex::new(whimpr_core::DictionaryStore::load(&dict_path())));
    let _ = BRAIN.set(Mutex::new(whimpr_core::brain::Brain::load(&brain_path())));
    let _ = STATS.set(Mutex::new(whimpr_core::StatsStore::load(&stats_path())));
    let _ = STYLE.set(Mutex::new(whimpr_core::StyleStore::load(&style_path())));
    let _ = OPENAI.set(Mutex::new(None));
    let _ = LOCAL.set(Mutex::new(None));
    rebuild_providers();

    // Load Whisper.
    std::thread::spawn(|| {
        let path = whisper_model_path();
        if !path.exists() {
            eprintln!("[whimpr:win] ASR model not found at {}", path.display());
            ASR_MODEL_MISSING.store(true, Ordering::SeqCst);
            return;
        }
        match whimpr_asr::WhisperEngine::load(&path) {
            Ok(engine) => {
                let _ = ASR.set(Arc::new(engine));
                eprintln!("[whimpr:win] ASR ready");
            }
            Err(e) => {
                eprintln!("[whimpr:win] ASR load failed: {e}");
                ASR_MODEL_MISSING.store(true, Ordering::SeqCst);
            }
        }
    });
    // Start the local cleanup worker.
    std::thread::spawn(|| {
        if let Some(w) = crate::local_llm::spawn_default() {
            if let Some(slot) = LOCAL.get() {
                *slot.lock().unwrap() = Some(w);
            }
        }
    });

    spawn_hook_thread();
    eprintln!("[whimpr:win] keyboard hook installed (push-to-talk: Right Ctrl)");
}

pub fn current_settings() -> whimpr_core::Settings {
    current_settings_inner()
}

pub fn update_settings(new: whimpr_core::Settings) {
    if let Some(m) = SETTINGS.get() {
        *m.lock().unwrap() = new.clone();
    }
    let _ = new.save(&settings_path());
    rebuild_providers();
}

pub fn rebuild_providers() {
    let settings = current_settings_inner();
    let model = settings.openai_model;
    let base_url = settings.openai_base_url;
    // See `whimpr_core::credentials` — a local file, not the OS keychain.
    let key = whimpr_core::CredentialStore::load(&credentials_path()).get("openai_api_key");
    if let Some(slot) = OPENAI.get() {
        *slot.lock().unwrap() = key.map(|k| {
            whimpr_cleanup::OpenAiProvider::with_base_url(k, model, Some(base_url))
        });
    }
}

pub fn stats_summary(tz_offset_minutes: i32) -> StatsSummary {
    STATS
        .get()
        .map(|m| m.lock().unwrap().summary(tz_offset_minutes, unix_now()))
        .unwrap_or_else(|| whimpr_core::StatsStore::default().summary(tz_offset_minutes, unix_now()))
}

pub fn history(limit: usize) -> Vec<whimpr_core::HistoryItem> {
    STATS.get().map(|m| m.lock().unwrap().history(limit)).unwrap_or_default()
}

pub fn style_summary(mode: whimpr_core::StyleMode) -> whimpr_core::StyleSummary {
    STYLE
        .get()
        .map(|m| m.lock().unwrap().summary(mode))
        .unwrap_or_else(|| whimpr_core::StyleStore::default().summary(mode))
}

pub fn style_summaries() -> std::collections::HashMap<whimpr_core::StyleMode, whimpr_core::StyleSummary> {
    whimpr_core::StyleMode::ALL.into_iter().map(|mode| (mode, style_summary(mode))).collect()
}

pub fn reset_style(mode: whimpr_core::StyleMode) {
    if let Some(m) = STYLE.get() {
        let mut store = m.lock().unwrap();
        store.reset(mode);
        let _ = store.save(&style_path());
    }
}

pub fn dictionary_entries() -> Vec<crate::hotkey::DictEntryDto> {
    DICTIONARY
        .get()
        .map(|m| {
            m.lock()
                .unwrap()
                .entries
                .iter()
                .map(|e| crate::hotkey::DictEntryDto {
                    correct: e.correct.clone(),
                    mishears: e.mishears.clone(),
                    auto: matches!(e.source, whimpr_core::DictSource::Auto),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn dictionary_add(correct: String, mishears: Vec<String>) {
    if let Some(m) = DICTIONARY.get() {
        let mut store = m.lock().unwrap();
        store.add(correct, mishears, whimpr_core::DictSource::Manual);
        let _ = store.save(&dict_path());
    }
}

pub fn brain_learn(pairs: Vec<(String, String)>) {
    if let Some(m) = BRAIN.get() {
        let mut brain = m.lock().unwrap();
        for (meant, heard) in pairs {
            brain.observe(&meant, &heard);
        }
        let _ = brain.save(&brain_path());
    }
}

pub fn brain_summary() -> whimpr_core::brain::Summary {
    BRAIN
        .get()
        .map(|m| m.lock().unwrap().summary())
        .unwrap_or_else(|| whimpr_core::brain::Brain::default().summary())
}

pub fn brain_reset() {
    if let Some(m) = BRAIN.get() {
        let mut brain = m.lock().unwrap();
        brain.reset();
        let _ = brain.save(&brain_path());
    }
}

// Recording a calibration line on Windows isn't wired up yet.
pub fn calibration_record_start() -> Result<(), String> {
    Err("Voice calibration recording isn't available on Windows yet.".into())
}

pub fn calibration_record_stop() -> Result<String, String> {
    Err("Voice calibration recording isn't available on Windows yet.".into())
}

pub fn dictionary_remove(correct: &str) {
    if let Some(m) = DICTIONARY.get() {
        let mut store = m.lock().unwrap();
        if store.remove(correct) {
            let _ = store.save(&dict_path());
        }
    }
}

pub fn dictionary_learn(correct: String, mishears: Vec<String>) {
    if let Some(m) = DICTIONARY.get() {
        let mut store = m.lock().unwrap();
        store.add(correct, mishears, whimpr_core::DictSource::Auto);
        let _ = store.save(&dict_path());
    }
}
