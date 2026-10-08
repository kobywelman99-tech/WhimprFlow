//! How much slower is the first transcription than the ones after it, and how much of that
//! does `warmup()` remove?
//!
//! Usage: cargo run --release -p whimpr-asr --example cold_vs_warm -- <model.bin> <audio.wav> [--warm]

use std::path::Path;
use std::time::Instant;

use whimpr_core::AsrEngine;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 3, "usage: cold_vs_warm <model.bin> <audio.wav> [--warm]");
    let warm = args.iter().any(|a| a == "--warm");

    let mut reader = hound::WavReader::open(&args[2])?;
    anyhow::ensure!(reader.spec().sample_rate == 16_000, "use a 16 kHz mono WAV");
    let pcm: Vec<f32> = reader.samples::<i16>().map(|s| s.map(|v| v as f32 / 32768.0)).collect::<Result<_, _>>()?;

    let started = Instant::now();
    let engine = whimpr_asr::WhisperEngine::load(Path::new(&args[1]))?;
    println!("load:    {:>6} ms", started.elapsed().as_millis());

    if warm {
        let started = Instant::now();
        engine.warmup()?;
        println!("warmup:  {:>6} ms", started.elapsed().as_millis());
    }
    for i in 1..=4 {
        let started = Instant::now();
        let text = engine.transcribe(&pcm)?.text;
        println!("decode {i}: {:>5} ms   ({} chars)", started.elapsed().as_millis(), text.len());
    }
    Ok(())
}
