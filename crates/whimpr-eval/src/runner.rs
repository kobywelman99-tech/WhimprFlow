//! Runs fixtures through the same steps as the app, after the microphone:
//! read → resample → (silence gate) → recognition with the dictionary prompt →
//! mishear fix → `whimpr_core::pipeline::run`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use whimpr_core::cleanup::CleanupLevel;
use whimpr_core::pipeline::{self, Env, Providers};
use whimpr_core::settings::{CleanupMode, Settings};

use crate::manifest::{Fixture, Manifest};
use crate::report::{FixtureResult, Status};
use crate::score;

/// Whole-clip peak below which the app doesn't even run recognition
/// (mirrors `hotkey.rs`; Task 4 moves this into `whimpr_core::audio_gate`).
pub const SILENCE_PEAK: f32 = 0.005;

/// Speech recognition, as the runner needs it.
pub trait Asr {
    fn transcribe(&self, pcm16k: &[f32], prompt: Option<&str>) -> anyhow::Result<String>;
}

impl Asr for whimpr_asr::WhisperEngine {
    fn transcribe(&self, pcm16k: &[f32], prompt: Option<&str>) -> anyhow::Result<String> {
        Ok(self.transcribe_with_prompt(pcm16k, prompt)?.text)
    }
}

pub struct Config {
    pub fixtures_dir: PathBuf,
    pub mode: CleanupMode,
    pub level: CleanupLevel,
    pub review: bool,
    /// How many times to run cleanup per clip (recognition runs once).
    pub runs: usize,
    /// Only fixtures whose id contains this.
    pub only: Option<String>,
}

/// Read a WAV (any rate, any channel count; integer or float) as mono f32.
pub fn read_wav(path: &Path) -> anyhow::Result<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>()?
        }
    };
    let mono = samples.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
    Ok((mono, spec.sample_rate))
}

pub fn list_missing<'a>(manifest: &'a Manifest, dir: &Path) -> Vec<&'a Fixture> {
    manifest.fixtures.iter().filter(|f| !dir.join(&f.wav).exists()).collect()
}

fn ms(since: Instant) -> u32 {
    since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// Everything a clip needs besides the clip itself.
struct Ctx<'a> {
    cfg: &'a Config,
    asr: &'a dyn Asr,
    dictionary: &'a whimpr_core::dictionary::DictionaryStore,
    prompt: Option<String>,
    settings: Settings,
    providers: Providers<'a>,
    cleanup_ran: bool,
}

pub fn run(manifest: &Manifest, cfg: &Config, asr: &dyn Asr, providers: Providers) -> Vec<FixtureResult> {
    let dictionary = manifest.dictionary_store();
    let ctx = Ctx {
        cfg,
        asr,
        dictionary: &dictionary,
        prompt: whimpr_asr::vocab_prompt(&dictionary.bias_words()),
        settings: Settings {
            cleanup_mode: cfg.mode,
            cleanup_level: cfg.level,
            review_enabled: cfg.review,
            ..Settings::default()
        },
        providers,
        cleanup_ran: !matches!(cfg.mode, CleanupMode::Raw) && !cfg.level.bypasses_llm(),
    };

    manifest
        .fixtures
        .iter()
        .filter(|f| cfg.only.as_deref().is_none_or(|s| f.id.contains(s)))
        .map(|fx| {
            let path = cfg.fixtures_dir.join(&fx.wav);
            if !path.exists() {
                return FixtureResult::missing(&fx.id, &fx.category);
            }
            run_one(fx, &path, &ctx).unwrap_or_else(|e| FixtureResult::errored(&fx.id, &fx.category, e.to_string()))
        })
        .collect()
}

fn run_one(fx: &Fixture, path: &Path, ctx: &Ctx) -> anyhow::Result<FixtureResult> {
    let mut result = FixtureResult::blank(&fx.id, &fx.category, Status::Ok);
    // A fixture with no golden text has nothing to approve.
    result.approved = fx.approved || fx.expected_cleaned.is_none();

    let (samples, rate) = read_wav(path)?;
    let started = Instant::now();
    let pcm = whimpr_audio::resample_to_16k(&samples, rate);
    result.timings.resample_ms = ms(started);

    let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let heard = if peak < SILENCE_PEAK {
        result.skipped_silent = true;
        String::new()
    } else {
        let started = Instant::now();
        let text = ctx.asr.transcribe(&pcm, ctx.prompt.as_deref())?;
        result.timings.asr_ms = ms(started);
        text
    };
    let raw = pipeline::fix_mishears(&heard, Some(ctx.dictionary), None);
    if let Some(expected) = &fx.expected_raw {
        result.raw_wer = Some(score::text_wer(expected, &raw));
    }
    result.raw = raw.clone();
    result.pipeline_ran = !raw.trim().is_empty() && ctx.cleanup_ran;

    // Cleanup is repeated `runs` times: it is the only nondeterministic stage.
    let runs = ctx.cfg.runs.max(1);
    let (mut passes, mut pipeline_total) = (0u32, 0u64);
    let mut checked = false;
    for i in 0..runs {
        let env = Env {
            settings: &ctx.settings,
            dictionary: ctx.dictionary,
            style_summary: None,
            app_bundle_id: None,
            providers: ctx.providers,
        };
        let started = Instant::now();
        let out = pipeline::run(&raw, &env);
        pipeline_total += u64::from(ms(started));
        if i == 0 {
            result.text = out.text.clone();
        }
        if let Some(verdict) = score::check_cleaned(fx, &out.text, ctx.cleanup_ran) {
            checked = true;
            if verdict.pass {
                passes += 1;
            } else if result.reasons.is_empty() {
                result.reasons = verdict.reasons;
            }
        }
    }
    result.timings.pipeline_ms = (pipeline_total / runs as u64) as u32;
    if checked {
        result.cleaned_pass = Some(passes as usize == runs);
        result.pass_runs = Some((passes, runs as u32));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::manifest::Manifest;

    /// Recognition that returns scripted text and records the prompt it was given.
    struct FakeAsr {
        text: &'static str,
        calls: RefCell<Vec<Option<String>>>,
    }

    impl FakeAsr {
        fn new(text: &'static str) -> Self {
            Self { text, calls: RefCell::new(Vec::new()) }
        }
    }

    impl Asr for FakeAsr {
        fn transcribe(&self, _: &[f32], prompt: Option<&str>) -> anyhow::Result<String> {
            self.calls.borrow_mut().push(prompt.map(String::from));
            Ok(self.text.to_string())
        }
    }

    fn write_wav(dir: &Path, name: &str, rate: u32, channels: u16, samples: &[f32]) {
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
        for s in samples {
            w.write_sample((s * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("whimpr-eval-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.05).sin()).collect()
    }

    fn config(dir: &Path) -> Config {
        Config { fixtures_dir: dir.into(), mode: CleanupMode::Raw, level: CleanupLevel::Light, review: false, runs: 1, only: None }
    }

    const MANIFEST: &str = r#"{
        "dictionary":[{"correct":"ChargeBee","mishears":["charge bee"]}],
        "fixtures":[
          {"id":"speech","wav":"speech.wav","category":"dictionary","say":"renew ChargeBee",
           "expected_raw":"renew ChargeBee"},
          {"id":"quiet","wav":"quiet.wav","category":"silence","say":"","expect":"no_output"},
          {"id":"absent","wav":"absent.wav","category":"filler","say":"hello"}
        ]}"#;

    #[test]
    fn a_clip_goes_through_recognition_the_dictionary_and_scoring() {
        let dir = temp_dir("flow");
        write_wav(&dir, "speech.wav", 16_000, 1, &tone(16_000, 0.3));
        write_wav(&dir, "quiet.wav", 16_000, 1, &vec![0.0; 48_000]);
        let manifest = Manifest::parse(MANIFEST).unwrap();
        let asr = FakeAsr::new("renew charge bee");

        let results = run(&manifest, &config(&dir), &asr, Providers::default());

        let speech = &results[0];
        assert_eq!(speech.status, Status::Ok);
        assert_eq!(speech.raw, "renew ChargeBee", "the dictionary's mishear fix is applied");
        assert_eq!(speech.raw_wer, Some(0.0));
        assert!(asr.calls.borrow()[0].as_deref().unwrap().contains("ChargeBee"), "the dictionary is the recognition prompt");
        assert_eq!(speech.cleaned_pass, None, "no cleanup ran, so cleaned-text expectations don't apply");

        let quiet = &results[1];
        assert!(quiet.skipped_silent, "silence never reaches recognition");
        assert_eq!(asr.calls.borrow().len(), 1);
        assert_eq!(quiet.cleaned_pass, Some(true), "no output is the right answer for silence");

        assert_eq!(results[2].status, Status::Missing, "a clip that isn't recorded yet is reported, not skipped");
        assert_eq!(list_missing(&manifest, &dir).len(), 1);
    }

    #[test]
    fn a_hallucination_on_noisy_silence_fails_the_no_output_check() {
        let dir = temp_dir("halluc");
        write_wav(&dir, "quiet.wav", 16_000, 1, &tone(16_000, 0.05));
        let manifest = Manifest::parse(
            r#"{"fixtures":[{"id":"q","wav":"quiet.wav","category":"silence","say":"","expect":"no_output"}]}"#,
        )
        .unwrap();
        let results = run(&manifest, &config(&dir), &FakeAsr::new("Thank you."), Providers::default());
        assert_eq!(results[0].cleaned_pass, Some(false));
        assert!(results[0].reasons[0].contains("Thank you."));
    }

    #[test]
    fn other_sample_rates_and_channel_counts_are_accepted() {
        let dir = temp_dir("formats");
        // 48 kHz stereo, one second.
        write_wav(&dir, "s.wav", 48_000, 2, &tone(96_000, 0.3));
        let (mono, rate) = read_wav(&dir.join("s.wav")).unwrap();
        assert_eq!((mono.len(), rate), (48_000, 48_000));
        let manifest = Manifest::parse(r#"{"fixtures":[{"id":"s","wav":"s.wav","category":"c","say":"x"}]}"#).unwrap();
        let asr = FakeAsr::new("x");
        let results = run(&manifest, &config(&dir), &asr, Providers::default());
        assert_eq!(results[0].status, Status::Ok);
    }

    #[test]
    fn a_broken_file_is_an_error_row_not_a_crash() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join("s.wav"), b"not a wav").unwrap();
        let manifest = Manifest::parse(r#"{"fixtures":[{"id":"s","wav":"s.wav","category":"c","say":"x"}]}"#).unwrap();
        let results = run(&manifest, &config(&dir), &FakeAsr::new("x"), Providers::default());
        assert_eq!(results[0].status, Status::Error);
        assert!(results[0].error.is_some());
    }

    #[test]
    fn only_filters_by_id() {
        let dir = temp_dir("only");
        write_wav(&dir, "speech.wav", 16_000, 1, &tone(16_000, 0.3));
        let manifest = Manifest::parse(MANIFEST).unwrap();
        let mut cfg = config(&dir);
        cfg.only = Some("speech".into());
        assert_eq!(run(&manifest, &cfg, &FakeAsr::new("x"), Providers::default()).len(), 1);
    }
}
