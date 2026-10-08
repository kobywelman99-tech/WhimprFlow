//! `cargo run -p whimpr-eval -- --help`

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use whimpr_core::cleanup::CleanupLevel;
use whimpr_core::pipeline::Providers;
use whimpr_core::settings::CleanupMode;
use whimpr_core::CleanupProvider;
use whimpr_eval::manifest::Manifest;
use whimpr_eval::report::{self, Meta, Report};
use whimpr_eval::runner::{self, Config};
use whimpr_eval::{recording, synth};

const HELP: &str = "\
whimpr-eval: run recorded clips through the dictation pipeline and report accuracy and latency

USAGE: cargo run -p whimpr-eval [--release] -- [OPTIONS]

  --fixtures DIR          clips and manifest.json (default eval/fixtures)
  --model PATH            whisper model (default: the best one in the app's models folder)
  --provider P            none (default; raw transcript only) | anthropic | openai
                          anthropic/openai read ANTHROPIC_API_KEY / OPENAI_API_KEY
  --level L               none | light (default) | medium | high
  --no-review             skip the sense check
  --show-text             also print what was heard and what would be pasted, per clip
  --runs N                run cleanup N times per clip, to see LLM variance (default 1)
  --only TEXT             only fixtures whose id contains TEXT
  --baseline FILE         compare with an earlier report
  --out FILE              write the report here (default eval/reports/<time>.json)
  --no-write              don't write a report
  --list-missing          list clips that haven't been recorded yet, then exit
  --write-recording FILE  write the recording script (RECORDING.md) from the manifest, then exit
  --synth DIR             write synthetic clips and a manifest to DIR, then exit
";

#[derive(Default)]
struct Args {
    fixtures: Option<PathBuf>,
    model: Option<PathBuf>,
    provider: String,
    level: Option<String>,
    no_review: bool,
    show_text: bool,
    runs: usize,
    only: Option<String>,
    baseline: Option<PathBuf>,
    out: Option<PathBuf>,
    no_write: bool,
    list_missing: bool,
    write_recording: Option<PathBuf>,
    synth: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args { provider: "none".into(), runs: 1, ..Args::default() };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--fixtures" => args.fixtures = Some(value("--fixtures")?.into()),
            "--model" => args.model = Some(value("--model")?.into()),
            "--provider" => args.provider = value("--provider")?,
            "--level" => args.level = Some(value("--level")?),
            "--no-review" => args.no_review = true,
            "--show-text" => args.show_text = true,
            "--runs" => args.runs = value("--runs")?.parse().context("--runs wants a number")?,
            "--only" => args.only = Some(value("--only")?),
            "--baseline" => args.baseline = Some(value("--baseline")?.into()),
            "--out" => args.out = Some(value("--out")?.into()),
            "--no-write" => args.no_write = true,
            "--list-missing" => args.list_missing = true,
            "--write-recording" => args.write_recording = Some(value("--write-recording")?.into()),
            "--synth" => args.synth = Some(value("--synth")?.into()),
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other => bail!("unknown option {other}\n\n{HELP}"),
        }
    }
    Ok(args)
}

fn models_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("WHIMPR_MODELS_DIR") {
        return dir.into();
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/WhimprFlow/models")
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("WhimprFlow/models")
    }
}

/// The same preference order the app uses (`hotkey.rs::model_path`).
fn default_model() -> Option<PathBuf> {
    let dir = models_dir();
    ["ggml-large-v3-turbo.bin", "ggml-medium.en.bin", "ggml-small.en.bin", "ggml-base.en.bin"]
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.exists())
}

fn parse_level(s: &str) -> Result<CleanupLevel> {
    Ok(match s {
        "none" => CleanupLevel::None,
        "light" => CleanupLevel::Light,
        "medium" => CleanupLevel::Medium,
        "high" => CleanupLevel::High,
        other => bail!("unknown level {other} (none, light, medium, high)"),
    })
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn stamp(unix: u64) -> String {
    let (y, m, d) = civil((unix / 86_400) as i64);
    let rem = unix % 86_400;
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn git_head() -> Option<String> {
    let out = std::process::Command::new("git").args(["rev-parse", "--short", "HEAD"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn load_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join("manifest.json");
    let json = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Manifest::parse(&json).with_context(|| format!("parsing {}", path.display()))
}

type Cloud = Option<Box<dyn CleanupProvider>>;

fn main() -> Result<()> {
    let args = parse_args()?;
    let fixtures_dir = args.fixtures.clone().unwrap_or_else(|| "eval/fixtures".into());

    if let Some(dir) = &args.synth {
        synth::write_all(dir)?;
        println!("wrote synthetic clips and manifest.json to {}", dir.display());
        return Ok(());
    }
    let manifest = load_manifest(&fixtures_dir)?;
    if let Some(path) = &args.write_recording {
        std::fs::write(path, recording::render(&manifest))?;
        println!("wrote {}", path.display());
        return Ok(());
    }
    if args.list_missing {
        let missing = runner::list_missing(&manifest, &fixtures_dir);
        for f in &missing {
            println!("{:<40} {}", f.wav, if f.say.is_empty() { "(say nothing)" } else { &f.say });
        }
        println!("\n{} of {} clips still to record", missing.len(), manifest.fixtures.len());
        return Ok(());
    }

    // Providers.
    let level = parse_level(args.level.as_deref().unwrap_or("light"))?;
    let (mode, openai, anthropic): (_, Cloud, Cloud) =
        match args.provider.as_str() {
            "none" => (CleanupMode::Raw, None, None),
            "anthropic" => {
                let key = std::env::var("ANTHROPIC_API_KEY").context("set ANTHROPIC_API_KEY")?;
                (CleanupMode::Anthropic, None, Some(Box::new(whimpr_cleanup::AnthropicProvider::new(key, "claude-haiku-4-5"))))
            }
            "openai" => {
                let key = std::env::var("OPENAI_API_KEY").context("set OPENAI_API_KEY")?;
                (CleanupMode::OpenAi, Some(Box::new(whimpr_cleanup::OpenAiProvider::new(key, "gpt-4o-mini"))), None)
            }
            "local" => bail!("--provider local arrives with the local worker supervisor (Task 5)"),
            other => bail!("unknown provider {other} (none, anthropic, openai)"),
        };
    let providers = Providers { openai: openai.as_deref(), anthropic: anthropic.as_deref(), local: None };

    // Recognition.
    let model_path = args
        .model
        .clone()
        .or_else(default_model)
        .context("no whisper model found: pass --model PATH or put one in the app's models folder (docs/MODELS.md)")?;
    eprintln!("loading {} …", model_path.display());
    let engine = whimpr_asr::WhisperEngine::load(&model_path)?;

    let cfg = Config {
        fixtures_dir: fixtures_dir.clone(),
        mode,
        level,
        review: !args.no_review,
        runs: args.runs.max(1),
        only: args.only.clone(),
    };
    let results = runner::run(&manifest, &cfg, &engine, providers);

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let meta = Meta {
        timestamp_unix: now,
        git: git_head(),
        asr: model_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        provider: args.provider.clone(),
        level: args.level.clone().unwrap_or_else(|| "light".into()),
        review: cfg.review,
        runs: cfg.runs as u32,
    };
    let report = Report::new(meta, results);
    print!("{}", report.render(args.show_text));

    if let Some(path) = &args.baseline {
        let before: Report = serde_json::from_str(&std::fs::read_to_string(path)?)
            .with_context(|| format!("reading baseline {}", path.display()))?;
        println!("\n{}", report::compare(&before, &report));
    }
    if !args.no_write {
        let out = args.out.clone().unwrap_or_else(|| PathBuf::from(format!("eval/reports/{}.json", stamp(now))));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
        println!("report written to {}", out.display());
    }
    Ok(())
}
