//! WhimprFlow Tauri shell.
//!
//! Runs as a macOS accessory (menu-bar) app: a tray item, a transparent
//! always-on-top Flow Bar overlay, and a hidden Hub window. This is the M0
//! skeleton — the sidecar supervisor, real state-machine bridge, and native
//! panel promotion arrive in later milestones. The overlay already listens for
//! `whimpr://flowbar/state`, so the tray demo items prove the event pipeline.

mod appctx;
mod autolearn;
mod diag;
mod hotkey;
mod local_llm;
mod models;
mod paste;
mod permissions;

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

const OVERLAY_LABEL: &str = "whimpr_bar";
const HUB_LABEL: &str = "main";

#[derive(Clone, Serialize)]
struct BarStatePayload {
    state: &'static str,
}

/// Anchor the overlay window bottom-center of its monitor — preferring
/// whichever monitor currently has the mouse cursor, so a multi-monitor setup
/// puts the pill on the screen the user is actually working on rather than
/// wherever it happened to launch. Falls back through primary/current/first
/// available if the cursor position can't be read (e.g. before the window
/// has mapped).
fn position_overlay(w: &WebviewWindow) {
    let cursor_monitor = w
        .cursor_position()
        .ok()
        .and_then(|p| w.monitor_from_point(p.x, p.y).ok().flatten());
    let monitor = cursor_monitor
        .or_else(|| w.primary_monitor().ok().flatten())
        .or_else(|| w.current_monitor().ok().flatten())
        .or_else(|| w.available_monitors().ok().and_then(|m| m.into_iter().next()));
    let Some(monitor) = monitor else {
        eprintln!("[whimpr] no monitor found — overlay stays at default position");
        return;
    };
    let scale = monitor.scale_factor();
    let msize = monitor.size();
    let mpos = monitor.position();
    let Ok(wsize) = w.outer_size() else { return };
    let inset = (40.0 * scale) as i32;
    let x = mpos.x + (msize.width as i32 - wsize.width as i32) / 2;
    let y = mpos.y + msize.height as i32 - wsize.height as i32 - inset;
    let _ = w.set_position(tauri::PhysicalPosition { x, y });
    eprintln!(
        "[whimpr] overlay placed: monitor {}x{} @({},{}) scale {:.1} -> window {}x{} @({},{})",
        msize.width, msize.height, mpos.x, mpos.y, scale, wsize.width, wsize.height, x, y
    );
}

fn build_overlay(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    let overlay = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("overlay.html".into()),
    )
    .title("WhimprBar")
    // Tight window so it only catches clicks right around the pill, not a big
    // invisible box over the app behind it.
    .inner_size(300.0, 72.0)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    // Not user-resizable (no drag handles) — the frontend still grows/shrinks
    // it programmatically via `resize_overlay` as the pill's content changes.
    .resizable(false)
    .visible(true)
    .build()?;
    // A normal window is confined to whichever Space it was created on; this
    // pins it to the "all Spaces" collection so it stays visible when the
    // user swipes to a different desktop/Space, like a HUD.
    let _ = overlay.set_visible_on_all_workspaces(true);
    // Once the user has dragged the pill somewhere, honor that instead of
    // resetting to the default bottom-center spot on every launch.
    match hotkey::current_settings().overlay_pos {
        Some((x, y)) => {
            let _ = overlay.set_position(tauri::PhysicalPosition { x, y });
        }
        None => position_overlay(&overlay),
    }
    let _ = overlay.show();
    Ok(overlay)
}

/// Move the overlay to the monitor under the cursor, bottom-centered — called
/// when a new dictation session starts so the pill is always on the screen
/// the user is actually working on, not wherever it happened to launch. A
/// no-op once the user has dragged the pill to a custom spot (same rule
/// `resize_overlay` already follows): a manual placement should stick, not
/// get silently overridden the next time they start dictating.
pub fn reposition_to_active_screen(app: &tauri::AppHandle) {
    if hotkey::current_settings().overlay_pos.is_some() {
        return;
    }
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        position_overlay(&w);
    }
}

/// Resize the overlay to fit its current content (the pill grows as the live
/// transcript grows). If the user hasn't dragged it, keep the default
/// behavior of re-anchoring bottom-center so it doesn't drift as its height
/// changes. Once they've dragged it to a custom spot, leave that top-left
/// corner alone on resize instead — otherwise every content-size change
/// would silently undo the move. Called from the frontend whenever its
/// measured content size changes.
#[tauri::command]
fn resize_overlay(app: tauri::AppHandle, width: f64, height: f64) {
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = w.set_size(tauri::LogicalSize::new(width, height));
        if hotkey::current_settings().overlay_pos.is_none() {
            position_overlay(&w);
        }
    }
}

/// Persist the overlay's position after the user drags it (see the
/// `data-tauri-drag-region` pill in the frontend, which calls this once the
/// drag ends). From then on the pill stays where they put it instead of
/// snapping back to the bottom on resize or relaunch.
#[tauri::command]
fn save_overlay_position(x: i32, y: i32) {
    let mut settings = hotkey::current_settings();
    settings.overlay_pos = Some((x, y));
    hotkey::update_settings(settings);
}

/// Undo a drag and go back to the default bottom-center anchor.
#[tauri::command]
fn reset_overlay_position(app: tauri::AppHandle) {
    let mut settings = hotkey::current_settings();
    settings.overlay_pos = None;
    hotkey::update_settings(settings);
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        position_overlay(&w);
    }
}

fn build_hub(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, HUB_LABEL, WebviewUrl::App("index.html".into()))
        .title("WhimprFlow")
        .inner_size(920.0, 640.0)
        .min_inner_size(720.0, 480.0)
        .visible(true)
        .build()
}

fn emit_bar_state(app: &tauri::AppHandle, state: &'static str) {
    let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarStatePayload { state });
}

#[tauri::command]
fn get_settings() -> whimpr_core::Settings {
    hotkey::current_settings()
}

#[tauri::command]
fn set_settings(settings: whimpr_core::Settings) {
    hotkey::update_settings(settings);
}

/// Aggregated dictation stats for the Hub dashboard. `tz_offset_minutes` is the
/// browser's `Date.getTimezoneOffset()` so "today"/streak match the user's clock.
#[tauri::command]
fn get_stats(tz_offset_minutes: i32) -> whimpr_core::StatsSummary {
    hotkey::stats_summary(tz_offset_minutes)
}

/// Recent dictations for the Hub Home history list (newest first).
#[tauri::command]
fn get_history() -> Vec<whimpr_core::HistoryItem> {
    hotkey::history(200)
}

/// Dictionary entries for the Hub Dictionary screen.
#[tauri::command]
fn get_dictionary() -> Vec<hotkey::DictEntryDto> {
    hotkey::dictionary_entries()
}

/// Add a manual dictionary entry (word + optional known mishears).
#[tauri::command]
fn add_dictionary_entry(correct: String, mishears: Vec<String>) {
    hotkey::dictionary_add(correct, mishears);
}

/// Remove a dictionary entry by its spelling.
#[tauri::command]
fn remove_dictionary_entry(correct: String) {
    hotkey::dictionary_remove(&correct);
}

// ── Voice calibration ───────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct CalibrationPromptDto {
    id: u32,
    tag: String,
    text: String,
}

#[derive(serde::Deserialize)]
struct CalibrationAttemptDto {
    expected: String,
    heard: String,
}

#[derive(serde::Serialize)]
struct BrainPatternDto {
    meant: String,
    heard: String,
    rate: f32,
    count: u32,
    opportunities: u32,
}

#[derive(serde::Serialize)]
struct BrainOverviewDto {
    observations: u32,
    learned_words: u32,
    patterns: Vec<BrainPatternDto>,
}

fn brain_overview_dto() -> BrainOverviewDto {
    let s = hotkey::brain_summary();
    BrainOverviewDto {
        observations: s.observations,
        learned_words: s.learned_words,
        patterns: s
            .patterns
            .into_iter()
            .map(|p| BrainPatternDto {
                meant: p.meant,
                heard: p.heard,
                rate: p.rate,
                count: p.count,
                opportunities: p.opportunities,
            })
            .collect(),
    }
}

/// Lines to read aloud, spanning every style; `seed` varies which come up.
#[tauri::command]
fn calibration_prompts(count: u32, seed: u64) -> Vec<CalibrationPromptDto> {
    whimpr_core::calibration::prompts(count as usize, seed)
        .into_iter()
        .map(|p| CalibrationPromptDto { id: p.id, tag: p.tag.to_string(), text: p.text.to_string() })
        .collect()
}

#[tauri::command]
fn calibration_record_start() -> Result<(), String> {
    hotkey::calibration_record_start()
}

/// Stop recording and return what the recogniser heard. Runs off the main
/// thread: transcription takes a moment.
#[tauri::command]
async fn calibration_record_stop() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(hotkey::calibration_record_stop)
        .await
        .map_err(|e| e.to_string())?
}

/// Teach the brain from a calibration session: what each line was meant to be
/// against what the recogniser heard.
#[tauri::command]
fn calibration_learn(attempts: Vec<CalibrationAttemptDto>) -> BrainOverviewDto {
    hotkey::brain_learn(attempts.into_iter().map(|a| (a.expected, a.heard)).collect());
    brain_overview_dto()
}

/// What the brain has learned about how the user sounds.
#[tauri::command]
fn get_brain() -> BrainOverviewDto {
    brain_overview_dto()
}

/// Forget everything the brain learned.
#[tauri::command]
fn reset_brain() -> BrainOverviewDto {
    hotkey::brain_reset();
    brain_overview_dto()
}

/// The learned style profiles (Email/General/Essay-Report) for the Hub's
/// Style page, keyed by mode.
#[tauri::command]
fn get_style_summaries() -> std::collections::HashMap<whimpr_core::StyleMode, whimpr_core::StyleSummary> {
    hotkey::style_summaries()
}

/// Wipe one mode's learned style profile.
#[tauri::command]
fn reset_style_profile(mode: whimpr_core::StyleMode) {
    hotkey::reset_style(mode);
}

/// Permission + capability status shown in the Hub.
///
/// The permission half is a live read every time (see `permissions::snapshot`);
/// nothing here is remembered between calls. The Hub no longer has to ask for it
/// on a timer either — `permissions::watch` pushes the same shape at it on
/// `whimpr://permissions` the moment macOS changes its mind.
#[derive(Clone, Serialize)]
struct StatusReport {
    accessibility: bool,
    microphone: bool,
    input_monitoring: bool,
    microphone_grant: permissions::Grant,
    charged_to: Option<String>,
    microphone_hint: Option<String>,
    has_openai_key: bool,
    has_anthropic_key: bool,
    /// A speech model is installed (whisper recognition can run).
    has_asr_model: bool,
}

#[tauri::command]
fn get_status() -> StatusReport {
    let p = permissions::snapshot();
    StatusReport {
        accessibility: p.accessibility,
        microphone: p.microphone,
        input_monitoring: p.input_monitoring,
        microphone_grant: p.microphone_grant,
        charged_to: p.charged_to,
        microphone_hint: p.microphone_hint,
        has_openai_key: has_key("openai_api_key"),
        has_anthropic_key: has_key("anthropic_api_key"),
        has_asr_model: models::asr_model_installed(),
    }
}

// ── Speech models ───────────────────────────────────────────────────────────

/// The downloadable speech models and which are installed / in use.
#[tauri::command]
fn list_models() -> Vec<models::ModelDto> {
    models::list()
}

/// Start downloading a model in the background; progress arrives as
/// `whimpr://models/progress` events.
#[tauri::command]
fn download_model(app: tauri::AppHandle, id: String) -> Result<(), String> {
    models::start(app, &id)
}

/// Switch the speech engine to an installed model (Parakeet, or back to whisper).
#[tauri::command]
fn use_model(id: String) -> Result<(), String> {
    models::use_model(&id)
}

/// Stop the running download. The part already fetched is kept for next time.
#[tauri::command]
fn cancel_model_download() -> bool {
    models::cancel()
}

/// The most recent loud diagnostic (permission/injection failure), if any —
/// lets the Hub show what went wrong even if it was opened after the fact.
/// See `diag::report`, called from the dictation pipeline whenever text
/// fails to reach the cursor.
#[tauri::command]
fn get_last_error() -> Option<diag::ErrorDto> {
    diag::last_error()
}

/// Where cloud cleanup API keys live — see `whimpr_core::credentials` for why
/// this is a restricted-permission local file rather than the OS keychain.
fn credentials_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home)
        .join("Library/Application Support/WhimprFlow")
        .join("credentials.json")
}

fn has_key(account: &str) -> bool {
    whimpr_core::CredentialStore::load(&credentials_path()).get(account).is_some()
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Request microphone access: trigger the native prompt (bundle has a usage string)
/// by briefly opening the input device, and open the Microphone settings pane.
#[tauri::command]
fn request_microphone() {
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(|| {
            if let Ok(h) = whimpr_audio::start(|_: &[f32]| {}) {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let _ = h.stop();
            }
        });
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
    }
}

/// Request Accessibility — the permission that makes the Fn key work in every app and
/// lets us type into other apps. Fire the native prompt, then open the pane.
#[tauri::command]
fn request_accessibility() {
    #[cfg(target_os = "macos")]
    {
        let _ = paste::prompt_accessibility();
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
    }
}

/// Request Input Monitoring (needed for the Fn key to be seen in every app, not
/// just while WhimprFlow is frontmost): register + prompt, then open the pane.
#[tauri::command]
fn request_input_monitoring() {
    #[cfg(target_os = "macos")]
    {
        let _ = paste::request_input_monitoring();
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent");
    }
}

/// Cancel the in-progress dictation (the pill's ✕ button) — discards the
/// audio, no paste, same as pressing Esc.
#[tauri::command]
fn cancel_dictation() {
    hotkey::cancel_dictation();
}

/// End the in-progress dictation (the pill's stop button) as if the trigger
/// key had been released — transcribes, cleans up, and pastes as usual.
#[tauri::command]
fn stop_dictation() {
    hotkey::stop_dictation();
}

/// Save (or clear, when empty) an API key in the local credentials file, then
/// rebuild providers so it takes effect immediately. See
/// `whimpr_core::credentials` for why this isn't the OS keychain.
#[tauri::command]
fn set_api_key(provider: String, key: String) -> Result<(), String> {
    let account = match provider.as_str() {
        "openai" => "openai_api_key",
        "anthropic" => "anthropic_api_key",
        _ => return Err(format!("unknown provider {provider}")),
    };
    let path = credentials_path();
    let mut store = whimpr_core::CredentialStore::load(&path);
    store.set(account, &key);
    store.save(&path).map_err(|e| e.to_string())?;
    // Read it straight back to catch a write/read mismatch immediately
    // rather than only discovering it later via a silently-false
    // `has_anthropic_key`/`has_openai_key` status.
    let readback = whimpr_core::CredentialStore::load(&path).get(account);
    if !key.trim().is_empty() && readback.is_none() {
        return Err("key was saved but could not be read back".to_string());
    }
    hotkey::rebuild_providers();
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            get_stats,
            get_history,
            get_dictionary,
            add_dictionary_entry,
            remove_dictionary_entry,
            calibration_prompts,
            calibration_record_start,
            calibration_record_stop,
            calibration_learn,
            get_brain,
            reset_brain,
            get_style_summaries,
            reset_style_profile,
            get_status,
            get_last_error,
            list_models,
            download_model,
            cancel_model_download,
            use_model,
            request_microphone,
            request_accessibility,
            request_input_monitoring,
            set_api_key,
            cancel_dictation,
            stop_dictation,
            resize_overlay,
            save_overlay_position,
            reset_overlay_position
        ])
        .setup(|app| {
            // Regular app: shows in the Dock with a normal, focusable main window.
            // (Can switch to a menu-bar-only accessory app later for the Wispr look.)
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            build_overlay(app)?;
            let hub = build_hub(app)?;
            let _ = hub.show();
            let _ = hub.set_focus();

            // Wire the Fn key to the pill via the real state machine.
            hotkey::install(app.handle().clone());

            // Keep the permission rows honest without the Hub having to be awake
            // to ask. This is what makes the setup screen's promise ("turns green
            // the moment macOS applies it — no relaunch needed") actually true:
            // the Hub's own timer stops within seconds of its window going away,
            // and the reader is granting from System Settings precisely then.
            permissions::watch(app.handle().clone());

            let open = MenuItem::with_id(app, "open", "Open WhimprFlow", true, None::<&str>)?;
            let demo_rec =
                MenuItem::with_id(app, "demo_rec", "Demo: recording", true, None::<&str>)?;
            let demo_idle = MenuItem::with_id(app, "demo_idle", "Demo: idle", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit WhimprFlow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &demo_rec, &demo_idle, &sep, &quit])?;

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window(HUB_LABEL) {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "demo_rec" => emit_bar_state(app, "recording"),
                    "demo_idle" => emit_bar_state(app, "idle"),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            tray.build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running WhimprFlow");
}
