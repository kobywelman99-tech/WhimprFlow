//! What to do with a recording before speech recognition: skip it if nobody spoke,
//! trim the quiet edges, and make sure it is long enough for whisper.
//!
//! The decisions are pure functions over per-frame speech probabilities, so they are
//! tested without a model. A [`SpeechDetector`] (Silero VAD, in `whimpr-vad`) supplies
//! the probabilities. Push-to-talk means the key release is the end of the utterance,
//! so this only trims and skips; it never decides when to stop.

/// Samples per detector frame: 30 ms at 16 kHz.
pub const FRAME_SAMPLES: usize = 480;
pub const SAMPLE_RATE: usize = 16_000;

/// whisper.cpp returns nothing for audio under 1000 ms ("input is too short"), so a
/// quick "yes" would vanish. Shorter clips are padded with silence up to this length.
pub const MIN_WHISPER_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 10;

/// Supplies one speech probability (0–1) per [`FRAME_SAMPLES`] of 16 kHz audio.
/// A trailing partial frame is zero-padded.
pub trait SpeechDetector {
    fn speech_probabilities(&mut self, pcm16k: &[f32]) -> anyhow::Result<Vec<f32>>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GatePolicy {
    /// Speech starts when a frame goes above this probability. 0.3 is `transcribe-rs`'s
    /// recommendation, and it matters: quiet one-word dictations ("bye", "you", "no") sit at
    /// 0.3-0.5 and never clear Silero's usual 0.5, while the non-speech we measured (white
    /// noise, clicks, bursts) peaks near 0.13. Losing a word is worse than letting noise
    /// through, because the transcript checks catch the latter.
    pub threshold: f32,
    /// Once speech has started, it continues while frames stay above this (lower)
    /// probability, so soft word endings aren't clipped.
    pub continue_threshold: f32,
    /// Speech shorter than this many frames in total is treated as a blip (a click), not an
    /// utterance. 4 frames is 120 ms: it removes the shortest transients and keeps "no".
    pub min_speech_frames: usize,
    /// Speech separated by less than this many frames is one utterance (about 600 ms).
    pub merge_gap_frames: usize,
    /// Quiet kept on each side of the speech, so soft consonants aren't clipped (about 150 ms).
    pub pad_frames: usize,
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self { threshold: 0.3, continue_threshold: 0.2, min_speech_frames: 4, merge_gap_frames: 20, pad_frames: 5 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Nothing that sounds like speech.
    NoSpeech,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Skip(SkipReason),
    /// Keep samples `start..end`; `speech_ms` is how much of that was speech.
    Keep { start: usize, end: usize, speech_ms: u32 },
}

/// Decide what to keep from `probs`, the detector's output for a clip of `total_samples`.
pub fn plan(probs: &[f32], total_samples: usize, policy: &GatePolicy) -> Plan {
    // Runs of consecutive speech frames, as (first, last) inclusive.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut in_speech = false;
    for (i, &p) in probs.iter().enumerate() {
        in_speech = p > if in_speech { policy.continue_threshold } else { policy.threshold };
        if in_speech {
            match runs.last_mut() {
                Some(last) if last.1 + 1 == i => last.1 = i,
                _ => runs.push((i, i)),
            }
        }
    }
    // Merge runs that are close together into one utterance.
    let mut clusters: Vec<(usize, usize, usize)> = Vec::new(); // first, last, speech frames
    for (first, last) in runs {
        let len = last - first + 1;
        match clusters.last_mut() {
            Some(c) if first - c.1 <= policy.merge_gap_frames => {
                c.1 = last;
                c.2 += len;
            }
            _ => clusters.push((first, last, len)),
        }
    }
    clusters.retain(|c| c.2 >= policy.min_speech_frames);
    let (Some(first), Some(last)) = (clusters.first(), clusters.last()) else {
        return Plan::Skip(SkipReason::NoSpeech);
    };

    let start = first.0.saturating_sub(policy.pad_frames) * FRAME_SAMPLES;
    let end = ((last.1 + 1 + policy.pad_frames) * FRAME_SAMPLES).min(total_samples);
    let speech_frames: usize = clusters.iter().map(|c| c.2).sum();
    Plan::Keep { start: start.min(end), end, speech_ms: (speech_frames * 30) as u32 }
}

/// Pad `pcm` with trailing silence up to [`MIN_WHISPER_SAMPLES`].
pub fn pad_for_whisper(pcm: &mut Vec<f32>) {
    if pcm.len() < MIN_WHISPER_SAMPLES {
        pcm.resize(MIN_WHISPER_SAMPLES, 0.0);
    }
}

/// The audio to recognise, or the reason there isn't any.
#[derive(Debug, Clone, PartialEq)]
pub enum Gated {
    Skip(SkipReason),
    Audio {
        pcm: Vec<f32>,
        /// How much of it was speech, when a detector ran.
        speech_ms: Option<u32>,
    },
}

/// Run the whole gate over a 16 kHz clip. Without a detector (or if it fails) the clip
/// is kept whole, so a broken detector can never lose a dictation.
pub fn gate(pcm: &[f32], detector: Option<&mut dyn SpeechDetector>, policy: &GatePolicy) -> Gated {
    let verdict = detector.and_then(|d| d.speech_probabilities(pcm).ok()).map(|p| plan(&p, pcm.len(), policy));
    let (mut audio, speech_ms) = match verdict {
        Some(Plan::Skip(reason)) => return Gated::Skip(reason),
        Some(Plan::Keep { start, end, speech_ms }) => (pcm[start..end].to_vec(), Some(speech_ms)),
        None => (pcm.to_vec(), None),
    };
    pad_for_whisper(&mut audio);
    Gated::Audio { pcm: audio, speech_ms }
}

/// What whisper tends to write for audio that has no speech in it. Single words someone
/// might really dictate ("you", "bye") are deliberately not here: a quick one-word
/// dictation has about as little speech as the silence this guards against.
const HALLUCINATIONS: &[&str] = &[
    "thank you",
    "thank you very much",
    "thanks for watching",
    "thank you for watching",
    "[blank_audio]",
    "[silence]",
    "(silence)",
    "[music]",
    "(music)",
];

/// Speech shorter than this can't plausibly be a real "thank you".
const HALLUCINATION_MAX_SPEECH_MS: u32 = 350;

/// Whether `text` is one of whisper's stock phrases for non-speech *and* the detector
/// heard too little speech for it to be real. A genuine "thank you" has more speech
/// than that, so it is never dropped.
pub fn is_hallucination(text: &str, speech_ms: u32) -> bool {
    if speech_ms >= HALLUCINATION_MAX_SPEECH_MS {
        return false;
    }
    let normalized: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '[' | ']' | '(' | ')' | '_'))
        .collect();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    HALLUCINATIONS.contains(&normalized.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` frames of probability `p`.
    fn frames(p: f32, n: usize) -> Vec<f32> {
        vec![p; n]
    }

    fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    fn samples(frames: usize) -> usize {
        frames * FRAME_SAMPLES
    }

    #[test]
    fn silence_is_skipped() {
        let probs = frames(0.01, 100);
        assert_eq!(plan(&probs, samples(100), &GatePolicy::default()), Plan::Skip(SkipReason::NoSpeech));
        assert_eq!(plan(&[], 0, &GatePolicy::default()), Plan::Skip(SkipReason::NoSpeech));
    }

    #[test]
    fn a_click_or_cough_is_skipped() {
        // Three frames (90 ms) of something in two seconds of quiet.
        let probs = cat(&[frames(0.0, 30), frames(0.9, 3), frames(0.0, 34)]);
        assert_eq!(plan(&probs, samples(67), &GatePolicy::default()), Plan::Skip(SkipReason::NoSpeech));
    }

    #[test]
    fn speech_is_kept_with_padding_and_the_quiet_edges_are_trimmed() {
        let probs = cat(&[frames(0.0, 40), frames(0.95, 30), frames(0.0, 40)]);
        let Plan::Keep { start, end, speech_ms } = plan(&probs, samples(110), &GatePolicy::default()) else {
            panic!("speech was skipped");
        };
        assert_eq!(start, samples(40 - 5), "5 frames of lead-in");
        assert_eq!(end, samples(40 + 30 + 5), "5 frames of tail");
        assert_eq!(speech_ms, 900);
    }

    #[test]
    fn padding_never_runs_off_the_clip() {
        let probs = cat(&[frames(0.95, 20), frames(0.0, 2)]);
        let Plan::Keep { start, end, .. } = plan(&probs, samples(22) - 100, &GatePolicy::default()) else {
            panic!("speech was skipped");
        };
        assert_eq!(start, 0);
        assert_eq!(end, samples(22) - 100);
    }

    #[test]
    fn a_fading_word_ending_stays_in_but_a_faint_sound_alone_never_starts_speech() {
        // Strong speech that trails off at 0.25: still speech under hysteresis.
        let fading = cat(&[frames(0.0, 10), frames(0.9, 12), frames(0.25, 8), frames(0.0, 10)]);
        let Plan::Keep { speech_ms, .. } = plan(&fading, samples(40), &GatePolicy::default()) else { panic!("skipped") };
        assert_eq!(speech_ms, 600, "12 strong frames plus the 8 fading ones");

        // The same 0.25 level on its own never crosses the start threshold.
        let faint = frames(0.25, 60);
        assert_eq!(plan(&faint, samples(60), &GatePolicy::default()), Plan::Skip(SkipReason::NoSpeech));
    }

    // Silero v4's probabilities, measured per 30 ms frame with its memory reset for each
    // clip, for one word read by a macOS voice. Quiet words hover at 0.3-0.5 and only just
    // cross 0.5, which is why the start threshold matters.
    const BYE: [f32; 19] = [0.189, 0.115, 0.061, 0.070, 0.219, 0.264, 0.329, 0.399, 0.435, 0.435, 0.518, 0.480, 0.048, 0.004, 0.001, 0.0, 0.0, 0.0, 0.0];
    const YOU: [f32; 17] = [0.204, 0.211, 0.203, 0.332, 0.457, 0.524, 0.471, 0.513, 0.390, 0.059, 0.004, 0.001, 0.001, 0.0, 0.0, 0.0, 0.0];
    const NO: [f32; 20] = [0.218, 0.196, 0.253, 0.287, 0.261, 0.237, 0.269, 0.317, 0.370, 0.421, 0.440, 0.354, 0.111, 0.013, 0.003, 0.001, 0.0, 0.0, 0.0, 0.0];
    const HI: [f32; 20] = [0.031, 0.036, 0.232, 0.459, 0.532, 0.678, 0.676, 0.682, 0.686, 0.691, 0.684, 0.670, 0.685, 0.200, 0.009, 0.004, 0.001, 0.0, 0.0, 0.0];

    #[test]
    fn a_quick_quiet_one_word_dictation_is_kept() {
        for (word, trace) in [("bye", &BYE[..]), ("you", &YOU[..]), ("no", &NO[..]), ("hi", &HI[..])] {
            let verdict = plan(trace, samples(trace.len()), &GatePolicy::default());
            assert!(matches!(verdict, Plan::Keep { .. }), "\"{word}\" was skipped: {verdict:?}");
        }
    }

    #[test]
    fn noise_level_probabilities_stay_skipped() {
        // White noise, clicks and bursts peaked at 0.13 when measured; none came close to a word.
        let faint: Vec<f32> = (0..60).map(|i| 0.04 + 0.09 * ((i * 7) % 10) as f32 / 10.0).collect();
        assert!(faint.iter().all(|&p| p <= 0.13));
        assert_eq!(plan(&faint, samples(60), &GatePolicy::default()), Plan::Skip(SkipReason::NoSpeech));
    }

    #[test]
    fn a_lone_you_or_bye_is_not_mistaken_for_a_hallucination() {
        for word in ["You", "you.", "Bye", "Bye bye."] {
            assert!(!is_hallucination(word, 200), "{word}");
        }
    }

    #[test]
    fn a_short_real_word_is_kept() {
        // Eleven frames (330 ms), like a quick "yes".
        let probs = cat(&[frames(0.0, 4), frames(0.9, 11), frames(0.0, 6)]);
        assert!(matches!(plan(&probs, samples(21), &GatePolicy::default()), Plan::Keep { .. }));
    }

    #[test]
    fn a_pause_inside_an_utterance_does_not_split_it() {
        // Speech, a 400 ms pause, speech: one utterance, the pause stays.
        let probs = cat(&[frames(0.0, 10), frames(0.9, 20), frames(0.1, 13), frames(0.9, 20), frames(0.0, 10)]);
        let Plan::Keep { start, end, speech_ms } = plan(&probs, samples(73), &GatePolicy::default()) else {
            panic!("speech was skipped");
        };
        assert_eq!((start, end), (samples(5), samples(68)));
        assert_eq!(speech_ms, 1200);
    }

    #[test]
    fn an_early_cough_before_the_real_speech_is_left_out() {
        let probs = cat(&[frames(0.9, 3), frames(0.0, 60), frames(0.9, 25), frames(0.0, 10)]);
        let Plan::Keep { start, .. } = plan(&probs, samples(98), &GatePolicy::default()) else {
            panic!("speech was skipped");
        };
        assert_eq!(start, samples(63 - 5), "starts at the real speech, not the cough");
    }

    #[test]
    fn short_clips_are_padded_to_what_whisper_accepts() {
        let mut quick = vec![0.5f32; 6_000];
        pad_for_whisper(&mut quick);
        assert_eq!(quick.len(), MIN_WHISPER_SAMPLES);
        assert!(quick[6_000..].iter().all(|&s| s == 0.0));
        assert!(quick.len() > SAMPLE_RATE, "strictly over one second");

        let mut long = vec![0.5f32; 40_000];
        pad_for_whisper(&mut long);
        assert_eq!(long.len(), 40_000);
    }

    /// Returns scripted probabilities and records that it was asked.
    struct Scripted(Vec<f32>, bool);

    impl SpeechDetector for Scripted {
        fn speech_probabilities(&mut self, _: &[f32]) -> anyhow::Result<Vec<f32>> {
            if self.1 {
                anyhow::bail!("model crashed");
            }
            Ok(self.0.clone())
        }
    }

    #[test]
    fn gate_skips_silence_and_trims_and_pads_speech() {
        let policy = GatePolicy::default();
        let clip = vec![0.1f32; samples(100)];

        let mut quiet = Scripted(frames(0.0, 100), false);
        assert_eq!(gate(&clip, Some(&mut quiet), &policy), Gated::Skip(SkipReason::NoSpeech));

        let mut speech = Scripted(cat(&[frames(0.0, 50), frames(0.9, 12), frames(0.0, 38)]), false);
        let Gated::Audio { pcm, speech_ms } = gate(&clip, Some(&mut speech), &policy) else { panic!("skipped") };
        assert_eq!(speech_ms, Some(360));
        assert_eq!(pcm.len(), MIN_WHISPER_SAMPLES, "(12 + 10) frames is under a second, so it is padded");
    }

    #[test]
    fn a_missing_or_broken_detector_never_loses_a_dictation() {
        let clip = vec![0.1f32; samples(60)];
        let policy = GatePolicy::default();
        let Gated::Audio { pcm, speech_ms } = gate(&clip, None, &policy) else { panic!("skipped") };
        assert_eq!((pcm.len(), speech_ms), (MIN_WHISPER_SAMPLES.max(clip.len()), None));
        let mut broken = Scripted(vec![], true);
        let Gated::Audio { speech_ms, .. } = gate(&clip, Some(&mut broken), &policy) else { panic!("skipped") };
        assert_eq!(speech_ms, None);
    }

    #[test]
    fn stock_non_speech_phrases_are_dropped_only_when_there_was_barely_any_speech() {
        for text in ["Thank you.", "thank you", "[BLANK_AUDIO]", "Thanks for watching!", "(music)"] {
            assert!(is_hallucination(text, 0), "{text}");
            assert!(is_hallucination(text, 300), "{text}");
        }
        // Someone really said it: the detector heard half a second of speech or more.
        assert!(!is_hallucination("Thank you.", 600));
        // Other text is never touched.
        assert!(!is_hallucination("Thank you for sending that over", 0));
        assert!(!is_hallucination("Yes.", 0));
        assert!(!is_hallucination("", 0));
    }
}
