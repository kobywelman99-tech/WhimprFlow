//! Parakeet speech recognition as an [`AsrEngine`].
//!
//! Compared with whisper here: much faster than `large-v3-turbo` on a CPU, adds its own
//! punctuation and capitalisation, and keeps filler words ("um") that whisper drops. It has no
//! prompt input, so the dictionary can't steer how names are heard; the "also heard as"
//! replacements still run afterwards, as for every engine.

use std::path::Path;
use std::sync::Mutex;

use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
use transcribe_rs::onnx::Quantization;
use whimpr_core::asr::{AsrCaps, AsrEngine, AsrEngineId, Transcript};
use whimpr_core::audio_gate::MIN_WHISPER_SAMPLES;

/// What the model folder must contain.
pub const REQUIRED_FILES: [&str; 4] =
    ["encoder-model.int8.onnx", "decoder_joint-model.int8.onnx", "nemo128.onnx", "vocab.txt"];

/// Required files that aren't in `dir`.
pub fn missing_files(dir: &Path) -> Vec<&'static str> {
    REQUIRED_FILES.iter().copied().filter(|f| !dir.join(f).is_file()).collect()
}

/// Audio too short to hold a word. The model is not asked about it.
fn is_blank(pcm16k: &[f32]) -> bool {
    pcm16k.len() < 1_600 // 100 ms
}

pub struct ParakeetEngine {
    // Decoding needs `&mut` access, and the app shares one engine between the live preview and
    // the final decode, so they take turns.
    model: Mutex<ParakeetModel>,
}

impl ParakeetEngine {
    /// Load the model from `dir` (the folder holding the four files).
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let missing = missing_files(dir);
        anyhow::ensure!(
            missing.is_empty(),
            "the Parakeet model in {} is incomplete; missing {}",
            dir.display(),
            missing.join(", ")
        );
        let model = ParakeetModel::load(dir, &Quantization::Int8)
            .map_err(|e| anyhow::anyhow!("loading the Parakeet model: {e}"))?;
        Ok(Self { model: Mutex::new(model) })
    }
}

impl AsrEngine for ParakeetEngine {
    fn id(&self) -> AsrEngineId {
        AsrEngineId::OnnxParakeet
    }

    fn caps(&self) -> AsrCaps {
        AsrCaps { supports_streaming: false }
    }

    /// One decode over a second of silence, so ONNX Runtime has built its execution plans
    /// before the first real dictation.
    fn warmup(&self) -> anyhow::Result<()> {
        self.transcribe(&vec![0.0; MIN_WHISPER_SAMPLES]).map(|_| ())
    }

    fn transcribe(&self, pcm16k: &[f32]) -> anyhow::Result<Transcript> {
        if is_blank(pcm16k) {
            return Ok(Transcript { text: String::new(), confidence: None });
        }
        let mut model = self.model.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let out = model
            .transcribe_with(pcm16k, &ParakeetParams::default())
            .map_err(|e| anyhow::anyhow!("Parakeet: {e}"))?;
        Ok(Transcript { text: out.text.trim().to_string(), confidence: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("whimpr-parakeet-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_files_are_listed_by_name() {
        let dir = temp("missing");
        assert_eq!(missing_files(&dir), REQUIRED_FILES);
        std::fs::write(dir.join("vocab.txt"), b"x").unwrap();
        std::fs::write(dir.join("nemo128.onnx"), b"x").unwrap();
        assert_eq!(missing_files(&dir), ["encoder-model.int8.onnx", "decoder_joint-model.int8.onnx"]);
    }

    #[test]
    fn loading_an_incomplete_folder_says_what_is_missing_instead_of_failing_obscurely() {
        let dir = temp("incomplete");
        std::fs::write(dir.join("vocab.txt"), b"x").unwrap();
        let err = ParakeetEngine::load(&dir).err().expect("must fail").to_string();
        assert!(err.contains("incomplete") && err.contains("encoder-model.int8.onnx"), "{err}");
        let err = ParakeetEngine::load(&dir.join("nope")).err().expect("must fail").to_string();
        assert!(err.contains("vocab.txt"), "a folder that doesn't exist is missing everything: {err}");
    }

    #[test]
    fn a_blip_too_short_to_hold_a_word_is_not_sent_to_the_model() {
        assert!(is_blank(&[]));
        assert!(is_blank(&vec![0.1; 1_599]));
        assert!(!is_blank(&vec![0.1; 1_600]));
    }

    #[test]
    fn the_downloader_fetches_exactly_the_files_the_engine_loads() {
        let registry = whimpr_models::builtin().into_iter().find(|m| m.id == whimpr_models::PARAKEET_ID).unwrap();
        let mut downloaded: Vec<&str> = registry.files.iter().map(|f| f.name.as_str()).collect();
        let mut required = REQUIRED_FILES.to_vec();
        downloaded.sort_unstable();
        required.sort_unstable();
        assert_eq!(downloaded, required);
    }
}
