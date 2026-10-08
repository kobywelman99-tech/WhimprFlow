//! Download a registry model with the real downloader, printing progress.
//!
//! Usage: cargo run --release -p whimpr-models --example fetch -- <model-id> <folder>
//! Ids: whisper-base-en, whisper-large-v3-turbo. Stop it with Ctrl-C and run it again to see it resume.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use whimpr_models::{builtin, download, Options, Progress};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(spec) = args.get(1).and_then(|id| builtin().into_iter().find(|m| &m.id == id)) else {
        eprintln!("usage: fetch <model-id> <folder>\nmodels: {}", builtin().iter().map(|m| m.id.clone()).collect::<Vec<_>>().join(", "));
        std::process::exit(2);
    };
    let folder = args.get(2).map(String::as_str).unwrap_or(".");
    let mut next_report = 0u64;
    let result = download(&spec, Path::new(folder), &Options::default(), &AtomicBool::new(false), &mut |p| match p {
        Progress::Downloading { downloaded, total } => {
            let percent = downloaded * 100 / total;
            if percent >= next_report {
                println!("{percent:>3}%  {} / {} MB", downloaded / 1_000_000, total / 1_000_000);
                next_report = percent + 10;
            }
        }
        Progress::Verifying => println!("verifying…"),
        Progress::Done => println!("done"),
    });
    match result {
        Ok(path) => println!("saved to {}", path.display()),
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
