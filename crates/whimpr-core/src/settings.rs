//! User settings, persisted as JSON. Drives the cleanup engine (which provider,
//! how aggressive) and other behavior. Kept dependency-light so it lives in core.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cleanup::{CleanupLevel, FormatMode};

/// Which cleanup engine processes transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupMode {
    /// Paste the raw transcript (no cleanup).
    Raw,
    /// Local on-device model (default — works offline, no API key).
    #[default]
    Local,
    /// OpenAI cloud.
    OpenAi,
    /// Anthropic cloud.
    Anthropic,
}

/// Persisted user configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub cleanup_mode: CleanupMode,
    pub cleanup_level: CleanupLevel,
    pub openai_model: String,
    /// API root for the "OpenAI" cleanup mode, e.g. `https://openrouter.ai/api/v1`
    /// to route through OpenRouter instead of OpenAI directly (same wire format).
    /// Empty string (the default) means OpenAI's own endpoint.
    #[serde(default)]
    pub openai_base_url: String,
    pub anthropic_model: String,
    /// Play the record-start ping.
    pub sound_on_start: bool,
    /// User-dragged overlay position (physical screen pixels, top-left), or
    /// `None` for the default bottom-center anchor. Set once the user drags
    /// the pill; from then on it stays put instead of snapping back to the
    /// bottom on every resize/restart.
    #[serde(default)]
    pub overlay_pos: Option<(i32, i32)>,
    /// Passively learn the user's writing/speaking voice from raw transcripts
    /// and fold a short summary into the (already-happening) cleanup request
    /// — see `crate::style`. On by default: it's zero extra API calls, only
    /// a small addition to the prompt of the cleanup call that already runs.
    #[serde(default = "default_true")]
    pub style_learning_enabled: bool,
    /// How to shape cleanup output for the target medium — `Auto` infers it
    /// from the focused app; a manual value (Email/General/Essay-Report)
    /// overrides that guess. See `crate::cleanup::FormatMode`.
    #[serde(default)]
    pub format_mode: FormatMode,
    /// Re-read each cleaned dictation as its recipient would and repair words that
    /// don't make sense, using what was heard (see `crate::review`). One extra
    /// cloud call per dictation; skipped for Raw and for the on-device model.
    #[serde(default = "default_true")]
    pub review_enabled: bool,
    /// Longest a single on-device cleanup request may take before the worker is restarted
    /// and the raw transcript is pasted instead.
    #[serde(default = "default_local_llm_timeout_ms")]
    pub local_llm_timeout_ms: u32,
}

fn default_local_llm_timeout_ms() -> u32 {
    3000
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cleanup_mode: CleanupMode::default(),
            cleanup_level: CleanupLevel::Light,
            openai_model: "gpt-4o-mini".to_string(),
            openai_base_url: String::new(),
            anthropic_model: "claude-haiku-4-5".to_string(),
            sound_on_start: true,
            overlay_pos: None,
            style_learning_enabled: true,
            format_mode: FormatMode::default(),
            review_enabled: true,
            local_llm_timeout_ms: default_local_llm_timeout_ms(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.cleanup_mode, CleanupMode::Local);
        assert_eq!(s.cleanup_level, CleanupLevel::Light);
    }

    #[test]
    fn an_old_settings_file_without_the_timeout_still_loads_with_the_default() {
        let old = r#"{"cleanup_mode":"anthropic","cleanup_level":"light","openai_model":"m","anthropic_model":"a","sound_on_start":true}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!((s.cleanup_mode, s.local_llm_timeout_ms), (CleanupMode::Anthropic, 3000));
    }

    #[test]
    fn round_trips_json() {
        let s = Settings {
            cleanup_mode: CleanupMode::Local,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cleanup_mode, CleanupMode::Local);
    }
}
