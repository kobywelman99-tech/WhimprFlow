//! What can be downloaded, and which model the app prefers when several are installed.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Speech recognition.
    Speech,
}

/// One file of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelFile {
    /// File name inside the model's folder, exactly what the engine looks for.
    pub name: String,
    pub url: String,
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSpec {
    /// Stable identifier the UI refers to.
    pub id: String,
    pub title: String,
    /// A few lines for the person choosing: speed, accuracy, trade-offs.
    pub blurb: String,
    pub role: Role,
    /// Subfolder of the models folder holding the files, or `None` for the models folder
    /// itself (a whisper model is one file straight in it; Parakeet is four files in a folder).
    pub folder: Option<String>,
    pub files: Vec<ModelFile>,
}

impl ModelSpec {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size_bytes).sum()
    }

    /// Where this model's files live under `models_dir`.
    pub fn install_dir(&self, models_dir: &Path) -> PathBuf {
        self.folder.as_ref().map_or_else(|| models_dir.to_path_buf(), |f| models_dir.join(f))
    }

    /// Every file is present at its expected size. (A cheap check: the hash is verified when
    /// the file is downloaded, not on every launch.)
    pub fn is_installed(&self, models_dir: &Path) -> bool {
        let dir = self.install_dir(models_dir);
        self.files.iter().all(|f| std::fs::metadata(dir.join(&f.name)).is_ok_and(|m| m.len() == f.size_bytes))
    }
}

/// Whisper models in the order the app prefers them. Bigger is more accurate, so if more than
/// one is installed the first present one is used. Only some of these can be downloaded here;
/// the others are recognised if someone placed them by hand (see docs/MODELS.md).
pub const WHISPER_PREFERENCE: [&str; 4] =
    ["ggml-large-v3-turbo.bin", "ggml-medium.en.bin", "ggml-small.en.bin", "ggml-base.en.bin"];

/// The model the app installs on first run.
pub const DEFAULT_ID: &str = "whisper-base-en";

/// The Parakeet model's id. It is the one speech model that isn't whisper.
pub const PARAKEET_ID: &str = "parakeet-tdt-v2";

/// Parakeet's files live in this folder inside the models folder.
const PARAKEET_FOLDER: &str = "parakeet-tdt-0.6b-v2-int8";

/// The Hugging Face revision the Parakeet hashes below were taken from, so the files can't
/// change underneath them.
const PARAKEET_REVISION: &str = "0bbb45a3365852604aef28b538a8f066f4ccaa85";

/// The models the app can download. Sizes and hashes are the ones Hugging Face publishes for
/// these files (confirmed against real downloads); a download that doesn't match is discarded,
/// so a changed upstream file fails safely instead of being loaded.
pub fn builtin() -> Vec<ModelSpec> {
    let whisper = |id: &str, title: &str, blurb: &str, file: &str, size: u64, sha: &str| ModelSpec {
        id: id.into(),
        title: title.into(),
        blurb: blurb.into(),
        role: Role::Speech,
        folder: None,
        files: vec![ModelFile {
            name: file.into(),
            url: format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}"),
            size_bytes: size,
            sha256: sha.into(),
        }],
    };
    let parakeet_file = |name: &str, size: u64, sha: &str| ModelFile {
        name: name.into(),
        url: format!("https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/{PARAKEET_REVISION}/{name}"),
        size_bytes: size,
        sha256: sha.into(),
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
            "The most accurate whisper model, especially with names and unusual words. Larger download.",
            "ggml-large-v3-turbo.bin",
            1_624_555_275,
            "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        ),
        ModelSpec {
            id: PARAKEET_ID.into(),
            title: "Parakeet (experimental)".into(),
            blurb: "NVIDIA's English recognizer: about twice as fast as Best, and it adds its own punctuation. \
                    It can't use your dictionary to hear names, so rely on \"also heard as\" entries. \
                    Model licensed CC-BY-4.0."
                .into(),
            role: Role::Speech,
            folder: Some(PARAKEET_FOLDER.into()),
            files: vec![
                parakeet_file("encoder-model.int8.onnx", 652_184_014, "3e0581fda6ab843888b51e56d7ee78b6d5bc3237ec113af1f732d1d5286aa155"),
                parakeet_file("decoder_joint-model.int8.onnx", 8_998_286, "a449f49acd68979d418651dd2dcb737cc0f1bf0225e009e29ee326354edbf7d3"),
                parakeet_file("nemo128.onnx", 139_764, "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f"),
                parakeet_file("vocab.txt", 9_384, "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d"),
            ],
        },
    ]
}

/// The most preferred whisper model present in `dir`, if any.
pub fn best_whisper_model(dir: &Path) -> Option<PathBuf> {
    WHISPER_PREFERENCE.iter().map(|f| dir.join(f)).find(|p| p.exists())
}

/// The Parakeet model's folder, if all its files are installed under `dir`.
pub fn parakeet_dir(dir: &Path) -> Option<PathBuf> {
    let spec = builtin().into_iter().find(|m| m.id == PARAKEET_ID)?;
    spec.is_installed(dir).then(|| spec.install_dir(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whimpr-models-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_registry_is_well_formed() {
        let models = builtin();
        let mut ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), models.len(), "ids are unique");
        for m in &models {
            assert!(!m.files.is_empty(), "{}", m.id);
            for f in &m.files {
                assert_eq!(f.sha256.len(), 64, "{} {}", m.id, f.name);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{} {}", m.id, f.name);
                assert!(f.url.starts_with("https://") && f.url.ends_with(&f.name), "{} {}", m.id, f.name);
                assert!(f.size_bytes > 1_000, "{} {}", m.id, f.name);
            }
            if m.id != PARAKEET_ID {
                assert_eq!(m.files.len(), 1, "a whisper model is one file");
                assert_eq!(m.folder, None);
                assert!(WHISPER_PREFERENCE.contains(&m.files[0].name.as_str()), "the app must be able to load {}", m.files[0].name);
            }
        }
        assert!(models.iter().any(|m| m.id == DEFAULT_ID));
    }

    #[test]
    fn parakeet_is_four_files_pinned_to_a_revision_in_its_own_folder() {
        let parakeet = builtin().into_iter().find(|m| m.id == PARAKEET_ID).expect("registered");
        assert_eq!(parakeet.files.len(), 4);
        assert_eq!(parakeet.folder.as_deref(), Some("parakeet-tdt-0.6b-v2-int8"));
        assert_eq!(parakeet.total_bytes(), 652_184_014 + 8_998_286 + 139_764 + 9_384);
        assert!(parakeet.files.iter().all(|f| f.url.contains(PARAKEET_REVISION)), "not the moving `main`");
        let names: Vec<_> = parakeet.files.iter().map(|f| f.name.as_str()).collect();
        for needed in ["encoder-model.int8.onnx", "decoder_joint-model.int8.onnx", "nemo128.onnx", "vocab.txt"] {
            assert!(names.contains(&needed), "the engine needs {needed}");
        }
    }

    #[test]
    fn the_most_accurate_installed_model_wins() {
        let dir = temp("pref");
        assert_eq!(best_whisper_model(&dir), None);
        std::fs::write(dir.join("ggml-base.en.bin"), b"x").unwrap();
        assert_eq!(best_whisper_model(&dir), Some(dir.join("ggml-base.en.bin")));
        std::fs::write(dir.join("ggml-small.en.bin"), b"x").unwrap();
        assert_eq!(best_whisper_model(&dir), Some(dir.join("ggml-small.en.bin")));
        std::fs::write(dir.join("ggml-large-v3-turbo.bin"), b"x").unwrap();
        assert_eq!(best_whisper_model(&dir), Some(dir.join("ggml-large-v3-turbo.bin")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parakeet_counts_as_installed_only_with_all_four_files_at_full_size() {
        let dir = temp("parakeet");
        let spec = builtin().into_iter().find(|m| m.id == PARAKEET_ID).unwrap();
        let folder = spec.install_dir(&dir);
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(parakeet_dir(&dir), None);
        // Sparse files: the right size without writing 650 MB.
        for (i, f) in spec.files.iter().enumerate() {
            let file = std::fs::File::create(folder.join(&f.name)).unwrap();
            file.set_len(f.size_bytes).unwrap();
            assert_eq!(parakeet_dir(&dir).is_some(), i == spec.files.len() - 1, "after {} files", i + 1);
        }
        std::fs::File::create(folder.join("vocab.txt")).unwrap().set_len(5).unwrap(); // truncated
        assert_eq!(parakeet_dir(&dir), None, "a truncated file breaks it");
        let _ = std::fs::remove_dir_all(dir);
    }
}
