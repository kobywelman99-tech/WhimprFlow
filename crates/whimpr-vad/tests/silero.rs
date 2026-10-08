//! The real model on real audio. `jfk.wav` is public-domain speech (see tests/data/README.md).

use whimpr_core::audio_gate::{gate, plan, GatePolicy, Gated, Plan, SkipReason, SpeechDetector, FRAME_SAMPLES};
use whimpr_vad::SileroDetector;

fn detector() -> SileroDetector {
    let dir = std::env::temp_dir().join(format!("whimpr-vad-test-{}", std::process::id()));
    SileroDetector::load(&dir).expect("the embedded model loads")
}

fn jfk() -> Vec<f32> {
    let mut reader = hound::WavReader::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/jfk.wav")).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
}

fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (16_000.0 * seconds) as usize]
}

#[test]
fn there_is_one_probability_per_frame_including_a_partial_last_one() {
    let mut d = detector();
    let pcm = vec![0.0; FRAME_SAMPLES * 3 + 100];
    assert_eq!(d.speech_probabilities(&pcm).unwrap().len(), 4);
    assert!(d.speech_probabilities(&[]).unwrap().is_empty());
}

#[test]
fn real_speech_is_detected() {
    let mut d = detector();
    let pcm = jfk();
    let probs = d.speech_probabilities(&pcm).unwrap();
    let speech = probs.iter().filter(|&&p| p > 0.5).count() as f32 / probs.len() as f32;
    // The excerpt has a long pause in the middle, so well under all of it is speech.
    assert!(speech > 0.45, "only {speech:.2} of the frames were speech");
    let Plan::Keep { speech_ms, .. } = plan(&probs, pcm.len(), &GatePolicy::default()) else { panic!("speech skipped") };
    assert!(speech_ms > 5_000, "{speech_ms} ms");
}

#[test]
fn silence_and_a_steady_tone_are_not_speech() {
    let mut d = detector();
    for (name, pcm) in [
        ("silence", silence(3.0)),
        ("tone", (0..48_000).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect()),
        ("faint noise", (0..48_000).map(|i| 0.01 * ((i * 7919 % 101) as f32 / 50.0 - 1.0)).collect()),
    ] {
        let probs = d.speech_probabilities(&pcm).unwrap();
        assert_eq!(plan(&probs, pcm.len(), &GatePolicy::default()), Plan::Skip(SkipReason::NoSpeech), "{name}");
    }
}

#[test]
fn quiet_before_and_after_the_speech_is_trimmed_away() {
    let mut d = detector();
    let speech = jfk();
    let mut clip = silence(3.0);
    clip.extend_from_slice(&speech);
    clip.extend(silence(3.0));

    let Gated::Audio { pcm, speech_ms } = gate(&clip, Some(&mut d), &GatePolicy::default()) else { panic!("skipped") };
    assert!(speech_ms.unwrap() > 5_000);
    // Roughly the speech plus a little padding: well short of the 17 s we gave it.
    assert!(pcm.len() < speech.len() + 16_000, "kept {} of {} samples", pcm.len(), clip.len());
    // The recording has about half a second of its own quiet at the edges, which is fair to trim.
    assert!(pcm.len() >= speech.len() - 16_000, "cut into the speech: kept {}", pcm.len());
    let peak = |s: &[f32]| s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert_eq!(peak(&pcm), peak(&speech), "the loudest part of the speech must survive");
}

#[test]
fn results_do_not_depend_on_what_was_processed_before() {
    let mut d = detector();
    let pcm = jfk();
    let first = d.speech_probabilities(&pcm).unwrap();
    let _ = d.speech_probabilities(&silence(2.0)).unwrap();
    let second = d.speech_probabilities(&pcm).unwrap();
    assert_eq!(first, second, "state must be reset between clips");
}
