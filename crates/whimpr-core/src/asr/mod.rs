//! The ASR seam. Per-OS backends implement [`AsrEngine`]: FluidAudio on the Apple
//! Neural Engine (macOS primary) and ONNX-Runtime Parakeet (Windows primary, macOS
//! fallback). The core only ever sees this trait; the engine choice is made behind it.

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Identifies which backend produced a transcript (for diagnostics + UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrEngineId {
    FluidAudioAne,
    OnnxParakeet,
    WhisperCpp,
}

/// A finalized transcription result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub confidence: Option<f32>,
}

/// Per-call hints for an engine. Engines ignore what they can't use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsrOptions {
    /// Spellings the recogniser should lean toward (the user's dictionary), for engines
    /// that take a prompt, such as whisper. Parakeet has no such input and ignores it.
    pub prompt: Option<String>,
}

/// Static capabilities of an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsrCaps {
    pub supports_streaming: bool,
}

/// Batch, finalize-on-release speech recognition over 16 kHz mono f32 samples in
/// [-1, 1]. Push-to-talk endpoints on key release, so a batch API is sufficient;
/// streaming preview is an optional per-engine capability.
pub trait AsrEngine: Send + Sync {
    fn id(&self) -> AsrEngineId;

    fn caps(&self) -> AsrCaps {
        AsrCaps {
            supports_streaming: false,
        }
    }

    /// Load the model and run a throwaway inference so the first real call is warm.
    fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Transcribe one complete utterance.
    fn transcribe(&self, pcm16k: &[f32]) -> anyhow::Result<Transcript>;

    /// Transcribe with hints. Engines that can use `options` override this; the default
    /// ignores them.
    fn transcribe_with(&self, pcm16k: &[f32], _options: &AsrOptions) -> anyhow::Result<Transcript> {
        self.transcribe(pcm16k)
    }
}

/// The engine in use, replaceable while the app runs (first-run download, switching
/// engines). A dictation takes an `Arc` of the engine when it starts, so swapping never
/// interrupts one already running: it finishes on the engine it began with.
#[derive(Default)]
pub struct AsrSlot {
    engine: RwLock<Option<Arc<dyn AsrEngine>>>,
}

impl AsrSlot {
    pub const fn new() -> Self {
        Self { engine: RwLock::new(None) }
    }

    /// The current engine, if one is loaded.
    pub fn get(&self) -> Option<Arc<dyn AsrEngine>> {
        self.engine.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn is_ready(&self) -> bool {
        self.get().is_some()
    }

    /// Put `engine` in use, returning the one it replaced.
    pub fn set(&self, engine: Arc<dyn AsrEngine>) -> Option<Arc<dyn AsrEngine>> {
        self.engine.write().unwrap_or_else(|poisoned| poisoned.into_inner()).replace(engine)
    }

    pub fn clear(&self) {
        *self.engine.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// Warm `engine` (so the first real dictation isn't the one that pays for the cold start),
/// then put it in use. A failed warm-up doesn't stop the engine being used; the error is
/// returned for the caller to log.
pub fn install(slot: &AsrSlot, engine: Arc<dyn AsrEngine>) -> Option<anyhow::Error> {
    let warmup = engine.warmup().err();
    slot.set(engine);
    warmup
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// An engine that answers with its own name and counts what it was asked.
    struct Named {
        name: &'static str,
        warmups: AtomicUsize,
        warmup_fails: bool,
        prompts: Mutex<Vec<Option<String>>>,
        uses_prompt: bool,
    }

    impl Named {
        fn new(name: &'static str) -> Arc<Self> {
            Arc::new(Self { name, warmups: AtomicUsize::new(0), warmup_fails: false, prompts: Mutex::default(), uses_prompt: false })
        }

        fn prompt_aware(name: &'static str) -> Arc<Self> {
            Arc::new(Self { uses_prompt: true, ..Arc::into_inner(Self::new(name)).unwrap() })
        }

        fn failing_warmup(name: &'static str) -> Arc<Self> {
            Arc::new(Self { warmup_fails: true, ..Arc::into_inner(Self::new(name)).unwrap() })
        }
    }

    impl AsrEngine for Named {
        fn id(&self) -> AsrEngineId {
            AsrEngineId::WhisperCpp
        }

        fn warmup(&self) -> anyhow::Result<()> {
            self.warmups.fetch_add(1, Ordering::SeqCst);
            if self.warmup_fails {
                anyhow::bail!("no GPU");
            }
            Ok(())
        }

        fn transcribe(&self, _: &[f32]) -> anyhow::Result<Transcript> {
            Ok(Transcript { text: self.name.to_string(), confidence: None })
        }

        fn transcribe_with(&self, pcm: &[f32], options: &AsrOptions) -> anyhow::Result<Transcript> {
            if self.uses_prompt {
                self.prompts.lock().unwrap().push(options.prompt.clone());
                return Ok(Transcript { text: format!("{}+prompt", self.name), confidence: None });
            }
            self.transcribe(pcm)
        }
    }

    fn text(engine: &Arc<dyn AsrEngine>) -> String {
        engine.transcribe(&[0.0]).unwrap().text
    }

    #[test]
    fn an_empty_slot_has_no_engine() {
        let slot = AsrSlot::new();
        assert!(slot.get().is_none());
        assert!(!slot.is_ready());
    }

    #[test]
    fn the_engine_in_the_slot_is_the_one_that_gets_used() {
        let slot = AsrSlot::new();
        assert!(slot.set(Named::new("base")).is_none(), "nothing was there before");
        assert!(slot.is_ready());
        assert_eq!(text(&slot.get().unwrap()), "base");
    }

    #[test]
    fn replacing_the_engine_hands_back_the_old_one_and_new_requests_use_the_new_one() {
        let slot = AsrSlot::new();
        slot.set(Named::new("base"));
        let old = slot.set(Named::new("turbo")).expect("the previous engine comes back");
        assert_eq!(text(&old), "base");
        assert_eq!(text(&slot.get().unwrap()), "turbo");
    }

    #[test]
    fn a_dictation_already_running_keeps_its_engine_when_the_slot_is_swapped() {
        let slot = AsrSlot::new();
        slot.set(Named::new("base"));
        let in_flight = slot.get().unwrap(); // what a dictation holds while it transcribes
        slot.set(Named::new("turbo"));
        assert_eq!(text(&in_flight), "base", "it finishes on the engine it started with");
        assert_eq!(text(&slot.get().unwrap()), "turbo", "the next one gets the new engine");
    }

    #[test]
    fn clearing_the_slot_removes_the_engine() {
        let slot = AsrSlot::new();
        slot.set(Named::new("base"));
        slot.clear();
        assert!(slot.get().is_none());
    }

    #[test]
    fn engines_without_prompt_support_ignore_the_prompt() {
        let engine: Arc<dyn AsrEngine> = Named::new("parakeet");
        let options = AsrOptions { prompt: Some("ChargeBee, Manvi".into()) };
        assert_eq!(engine.transcribe_with(&[0.0], &options).unwrap().text, "parakeet");
    }

    #[test]
    fn engines_that_support_a_prompt_receive_it() {
        let engine = Named::prompt_aware("whisper");
        let options = AsrOptions { prompt: Some("ChargeBee, Manvi".into()) };
        let as_dyn: Arc<dyn AsrEngine> = engine.clone();
        assert_eq!(as_dyn.transcribe_with(&[0.0], &options).unwrap().text, "whisper+prompt");
        assert_eq!(engine.prompts.lock().unwrap().as_slice(), [Some("ChargeBee, Manvi".to_string())]);
    }

    #[test]
    fn an_engine_is_warmed_before_it_is_published() {
        let slot = AsrSlot::new();
        let engine = Named::new("base");
        assert!(install(&slot, engine.clone()).is_none(), "no warm-up problem to report");
        assert_eq!(engine.warmups.load(Ordering::SeqCst), 1);
        assert!(slot.is_ready());
    }

    #[test]
    fn a_failed_warmup_is_reported_but_the_engine_is_still_usable() {
        let slot = AsrSlot::new();
        let error = install(&slot, Named::failing_warmup("base")).expect("the warm-up failure is returned");
        assert!(error.to_string().contains("no GPU"));
        assert_eq!(text(&slot.get().unwrap()), "base", "the first dictation just pays for the cold start");
    }

    #[test]
    fn the_slot_can_be_read_while_it_is_being_swapped() {
        let slot = Arc::new(AsrSlot::new());
        slot.set(Named::new("a"));
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let slot = slot.clone();
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        let t = text(&slot.get().expect("never empty while swapping"));
                        assert!(t == "a" || t == "b", "{t}");
                    }
                })
            })
            .collect();
        for i in 0..200 {
            slot.set(Named::new(if i % 2 == 0 { "b" } else { "a" }));
        }
        for r in readers {
            r.join().unwrap();
        }
    }
}
