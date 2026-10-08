//! What can be downloaded, and which model the app prefers when several are installed.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Speech recognition (whisper.cpp).
    Speech,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSpec {
    /// Stable identifier the UI refers to.
    pub id: String,
    pub title: String,
    /// One line for the person choosing: speed and accuracy.
    pub blurb: String,
    /// File name inside the models folder, exactly what the app looks for.
    pub file_name: String,
    pub url: String,
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
    pub role: Role,
}

/// Whisper models in the order the app prefers them. Bigger is more accurate, so if more than
/// one is installed the first present one is used. Only some of these can be downloaded here;
/// the others are recognised if someone placed them by hand (see docs/MODELS.md).
pub const WHISPER_PREFERENCE: [&str; 4] =
    ["ggml-large-v3-turbo.bin", "ggml-medium.en.bin", "ggml-small.en.bin", "ggml-base.en.bin"];

/// The model the app installs on first run.
pub const DEFAULT_ID: &str = "whisper-base-en";

/// The models the app can download. Sizes and hashes are the ones Hugging Face publishes for
/// these files; a download that doesn't match is discarded, so a changed upstream file fails
/// safely instead of being loaded.
pub fn builtin() -> Vec<ModelSpec> {
    let whisper = |id: &str, title: &str, blurb: &str, file: &str, size: u64, sha: &str| ModelSpec {
        id: id.into(),
        title: title.into(),
        blurb: blurb.into(),
        file_name: file.into(),
        url: format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}"),
        size_bytes: size,
        sha256: sha.into(),
        role: Role::Speech,
    };
    vec![
        whisper(
            DEFAULT_ID,
            "Fast",
            "Ready in about a minute and quick on any Mac. Good for everyday dictation.",
            "ggml-base.en.bin",
            147_964_211,
            "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        ),
        whisper(
            "whisper-large-v3-turbo",
            "Best",
            "The most accurate, especially with names and unusual words. Larger download.",
            "ggml-large-v3-turbo.bin",
            1_624_555_275,
            "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        ),
    ]
}

/// The most preferred whisper model present in `dir`, if any.
pub fn best_whisper_model(dir: &Path) -> Option<PathBuf> {
    WHISPER_PREFERENCE.iter().map(|f| dir.join(f)).find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_well_formed() {
        let models = builtin();
        let mut ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), models.len(), "ids are unique");
        for m in &models {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}", m.id);
            assert!(m.url.starts_with("https://") && m.url.ends_with(&m.file_name), "{}", m.id);
            assert!(m.size_bytes > 1_000_000, "{}", m.id);
            assert!(WHISPER_PREFERENCE.contains(&m.file_name.as_str()), "the app must be able to load {}", m.file_name);
        }
        assert!(models.iter().any(|m| m.id == DEFAULT_ID));
    }

    #[test]
    fn the_most_accurate_installed_model_wins() {
        let dir = std::env::temp_dir().join(format!("whimpr-models-pref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(best_whisper_model(&dir), None);
        std::fs::write(dir.join("ggml-base.en.bin"), b"x").unwrap();
        assert_eq!(best_whisper_model(&dir), Some(dir.join("ggml-base.en.bin")));
        std::fs::write(dir.join("ggml-small.en.bin"), b"x").unwrap();
        assert_eq!(best_whisper_model(&dir), Some(dir.join("ggml-small.en.bin")));
        std::fs::write(dir.join("ggml-large-v3-turbo.bin"), b"x").unwrap();
        assert_eq!(best_whisper_model(&dir), Some(dir.join("ggml-large-v3-turbo.bin")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
