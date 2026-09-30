//! The user-selectable dictation "Format Mode": `Auto` (the existing
//! app-detected default, see [`super::prompts::format_mode_for_app`]) or a
//! manual override that always wins over the focused app. Persisted on
//! [`crate::settings::Settings`]; resolved into prompt text in
//! [`super::prompts::system_for`] and into a learned-style bucket in
//! [`crate::style::resolve_style_mode`].

use serde::{Deserialize, Serialize};

/// How the target format for a dictation is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FormatMode {
    /// Infer from the focused app (see `prompts::format_mode_for_app`).
    #[default]
    Auto,
    /// Always format for email, regardless of the focused app.
    Email,
    /// Always use plain, unstructured prose — suppresses any app-based
    /// adaptation even inside an email/chat app.
    General,
    /// Always format for a formal essay or report.
    EssayReport,
}
