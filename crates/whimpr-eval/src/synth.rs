//! Synthetic clips for checking the plumbing before real recordings exist.
//! These say nothing about accuracy: they only prove the harness runs end to end.

use std::path::Path;

fn write(path: &Path, samples: impl Iterator<Item = f32>) -> anyhow::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

/// Writes three clips and a matching `manifest.json` into `dir`.
pub fn write_all(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    write(&dir.join("silence-3s.wav"), std::iter::repeat_n(0.0, 48_000))?;
    // Faint room tone: under the app's silence threshold.
    write(&dir.join("room-tone-3s.wav"), (0..48_000).map(|i| 0.002 * ((i * 7919 % 101) as f32 / 50.0 - 1.0)))?;
    write(&dir.join("tone-1s.wav"), (0..16_000).map(|i| 0.3 * (i as f32 * 0.05).sin()))?;
    std::fs::write(
        dir.join("manifest.json"),
        r#"{
  "fixtures": [
    {"id": "synth-silence", "wav": "silence-3s.wav", "category": "silence", "say": "", "expect": "no_output"},
    {"id": "synth-room-tone", "wav": "room-tone-3s.wav", "category": "silence", "say": "", "expect": "no_output"},
    {"id": "synth-tone", "wav": "tone-1s.wav", "category": "tone", "say": "(a tone, not speech)"}
  ]
}
"#,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{manifest::Manifest, runner};

    #[test]
    fn the_synthetic_set_is_readable_and_the_silent_clips_stay_below_the_gate() {
        let dir = std::env::temp_dir().join(format!("whimpr-eval-synth-{}", std::process::id()));
        write_all(&dir).unwrap();
        let manifest = Manifest::parse(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        assert!(runner::list_missing(&manifest, &dir).is_empty());
        for name in ["silence-3s.wav", "room-tone-3s.wav"] {
            let (pcm, _) = runner::read_wav(&dir.join(name)).unwrap();
            let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak < runner::SILENCE_PEAK, "{name} peak {peak}");
        }
        let (tone, _) = runner::read_wav(&dir.join("tone-1s.wav")).unwrap();
        assert!(tone.iter().fold(0.0f32, |m, s| m.max(s.abs())) > runner::SILENCE_PEAK);
    }
}
