//! Local speech-to-text via whisper.cpp (whisper-rs), implementing
//! [`whimpr_core::AsrEngine`]. Expects 16 kHz mono f32 samples.

use std::path::Path;

use whimpr_core::asr::{AsrCaps, AsrEngine, AsrEngineId, Transcript};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// A loaded whisper model ready to transcribe utterances.
pub struct WhisperEngine {
    ctx: WhisperContext,
}

impl WhisperEngine {
    /// Load a GGML/GGUF whisper model from `model_path`.
    pub fn load(model_path: &Path) -> anyhow::Result<Self> {
        let path = model_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("model path is not valid UTF-8"))?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("failed to load whisper model: {e}"))?;
        Ok(Self { ctx })
    }

    /// Like [`AsrEngine::transcribe`], but primes whisper with `prompt` (the
    /// user's vocabulary), which nudges it toward those spellings.
    pub fn transcribe_with_prompt(&self, pcm16k: &[f32], prompt: Option<&str>) -> anyhow::Result<Transcript> {
        self.transcribe_inner(pcm16k, prompt)
    }
}

/// Build a whisper `initial_prompt` from dictionary spellings: a short comma
/// list, capped (whisper only reads the last ~220 tokens of a prompt, and a
/// long one crowds out the audio).
pub fn vocab_prompt(words: &[String]) -> Option<String> {
    const MAX_CHARS: usize = 400;
    let mut out = String::new();
    for w in words.iter().map(|w| w.trim()).filter(|w| !w.is_empty()) {
        if out.len() + w.len() + 2 > MAX_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(w);
    }
    (!out.is_empty()).then_some(out)
}

impl AsrEngine for WhisperEngine {
    fn id(&self) -> AsrEngineId {
        AsrEngineId::WhisperCpp
    }

    fn caps(&self) -> AsrCaps {
        AsrCaps {
            supports_streaming: false,
        }
    }

    fn transcribe(&self, pcm16k: &[f32]) -> anyhow::Result<Transcript> {
        self.transcribe_inner(pcm16k, None)
    }
}

impl WhisperEngine {
    fn transcribe_inner(&self, pcm16k: &[f32], prompt: Option<&str>) -> anyhow::Result<Transcript> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("whisper create_state: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        // Push-to-talk utterances are always one short clip, not long-form audio.
        // Without this, whisper.cpp can split it into multiple internal segments
        // that repeat the same words — which then get concatenated below,
        // producing the sentence twice. Single-segment mode avoids that.
        params.set_single_segment(true);
        params.set_no_context(true);
        if let Some(prompt) = prompt {
            params.set_initial_prompt(prompt);
        }

        state
            .full(params, pcm16k)
            .map_err(|e| anyhow::anyhow!("whisper full: {e}"))?;

        let n = state
            .full_n_segments()
            .map_err(|e| anyhow::anyhow!("whisper n_segments: {e}"))?;
        let mut text = String::new();
        for i in 0..n {
            if let Ok(seg) = state.full_get_segment_text(i) {
                text.push_str(&seg);
            }
        }

        Ok(Transcript {
            text: text.trim().to_string(),
            confidence: None,
        })
    }
}
