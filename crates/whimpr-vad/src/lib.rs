//! Silero VAD (v4, MIT) as a [`SpeechDetector`].

use std::path::Path;

use transcribe_rs::vad::{SileroVad, Vad};
use whimpr_core::audio_gate::{SpeechDetector, FRAME_SAMPLES};

/// The model, embedded so the app never needs to download it.
static MODEL: &[u8] = include_bytes!("../models/silero_vad_v4.onnx");
const MODEL_FILE: &str = "silero_vad_v4.onnx";

/// The threshold `SileroVad` uses for its own yes/no; we use raw probabilities instead,
/// so this only needs to be a valid value.
const UNUSED_THRESHOLD: f32 = 0.5;

pub struct SileroDetector {
    vad: SileroVad,
}

impl SileroDetector {
    /// Load the model, first writing the embedded copy into `dir` (ONNX Runtime wants a
    /// file path). An existing file of the right size is reused.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(MODEL_FILE);
        let current = std::fs::metadata(&path).map(|m| m.len()).ok();
        if current != Some(MODEL.len() as u64) {
            std::fs::write(&path, MODEL)?;
        }
        let vad = SileroVad::new(&path, UNUSED_THRESHOLD).map_err(|e| anyhow::anyhow!("loading the VAD model: {e}"))?;
        Ok(Self { vad })
    }
}

impl SpeechDetector for SileroDetector {
    fn speech_probabilities(&mut self, pcm16k: &[f32]) -> anyhow::Result<Vec<f32>> {
        // Each clip starts with fresh memory: the model carries state from frame to frame.
        self.vad.reset();
        let mut probs = Vec::with_capacity(pcm16k.len() / FRAME_SAMPLES + 1);
        let mut frame = [0.0f32; FRAME_SAMPLES];
        for chunk in pcm16k.chunks(FRAME_SAMPLES) {
            frame[..chunk.len()].copy_from_slice(chunk);
            frame[chunk.len()..].fill(0.0);
            probs.push(self.vad.speech_probability(&frame).map_err(|e| anyhow::anyhow!("VAD inference: {e}"))?);
        }
        Ok(probs)
    }
}
