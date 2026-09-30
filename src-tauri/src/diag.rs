//! Bridge from `whimpr_core::diagnostics` to the UI.
//!
//! Turns an [`InjectionFailure`] into a loud, visible signal instead of the
//! `eprintln!`-only diagnostics the dictation loop used to have: the flow bar
//! pill switches to its `error` state and lingers long enough to read, every
//! window gets a `whimpr://error` broadcast, and the message is remembered so
//! the Hub can show it via [`last_error`] even if a window wasn't open (or
//! wasn't listening) at the moment it happened.

use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use whimpr_core::diagnostics::InjectionFailure;

/// A poisoned `Mutex` still holds valid data — recovering it means a panic
/// elsewhere doesn't also take down every future diagnostic report.
trait PoisonRecover<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T>;
}
impl<T> PoisonRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(target_os = "macos")]
const PLATFORM: whimpr_core::diagnostics::Platform = whimpr_core::diagnostics::Platform::MacOs;
#[cfg(target_os = "windows")]
const PLATFORM: whimpr_core::diagnostics::Platform = whimpr_core::diagnostics::Platform::Windows;
// Diagnostics aren't wired up on other targets (see hotkey::other stubs), but
// this constant must still exist for the crate to build there.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PLATFORM: whimpr_core::diagnostics::Platform = whimpr_core::diagnostics::Platform::MacOs;

const OVERLAY_LABEL: &str = "whimpr_bar";
/// How long the error stays on the pill before it reverts to idle — much
/// longer than the ~500ms "done" flash, since this is the one state the user
/// actually needs time to read.
const ERROR_LINGER_MS: u64 = 4500;

static LAST_ERROR: OnceLock<Mutex<Option<ErrorDto>>> = OnceLock::new();

#[derive(Clone, Serialize)]
pub struct ErrorDto {
    pub headline: String,
    pub detail: String,
}

#[derive(Clone, Serialize)]
struct BarPayload {
    state: &'static str,
}

/// Report a failure: log it, push the pill to the `error` state, broadcast
/// the message to every window, and remember it for [`last_error`].
#[allow(dead_code)] // used on macOS/Windows; inert-but-present on other targets
pub fn report(app: &AppHandle, failure: InjectionFailure) {
    let diag = failure.diagnose(PLATFORM);
    eprintln!("[whimpr] ⚠ {}: {}", diag.headline, diag.detail);
    let dto = ErrorDto { headline: diag.headline, detail: diag.detail };
    *LAST_ERROR.get_or_init(|| Mutex::new(None)).lock_recover() = Some(dto.clone());
    let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarPayload { state: "error" });
    let _ = app.emit("whimpr://error", dto);

    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ERROR_LINGER_MS));
        let _ = app2.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarPayload { state: "idle" });
    });
}

/// Report a non-blocking notice: something degraded but text still made it to
/// the cursor (e.g. cleanup was skipped and the raw transcript was pasted
/// instead) — as opposed to [`report`], which is for the pipeline actually
/// failing to deliver text. Unlike `report`, this does NOT hijack the pill
/// into its `error` state (that would misrepresent a successful-but-uncleaned
/// paste as a failure, and would cut the normal "done" flash short); it only
/// remembers the message for the Hub and broadcasts `whimpr://error` so an
/// open Hub window can show it immediately too.
pub fn notice(app: &AppHandle, headline: impl Into<String>, detail: impl Into<String>) {
    let dto = ErrorDto { headline: headline.into(), detail: detail.into() };
    eprintln!("[whimpr] ⓘ {}: {}", dto.headline, dto.detail);
    *LAST_ERROR.get_or_init(|| Mutex::new(None)).lock_recover() = Some(dto.clone());
    let _ = app.emit("whimpr://error", dto);
}

/// The most recent diagnostic, so the Hub can show it even after the pill
/// has already reverted to idle (e.g. Settings was opened after the fact).
pub fn last_error() -> Option<ErrorDto> {
    LAST_ERROR.get().and_then(|m| m.lock_recover().clone())
}

/// Clear the remembered error — called after a dictation succeeds, so a
/// long-past failure doesn't linger forever in the Hub.
#[allow(dead_code)]
pub fn clear_last_error() {
    if let Some(m) = LAST_ERROR.get() {
        *m.lock_recover() = None;
    }
}
