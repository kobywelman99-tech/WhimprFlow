//! A stand-in for `whimpr-llm-worker`, used by the supervisor's tests. It speaks the same
//! line protocol and misbehaves on demand. argv[1] (where the real worker gets its model
//! path) is the behavior, `mode` or `mode:param`:
//!
//! - `ok`                  print ready, answer every request with `{"text":"OK"}`
//! - `slow:MS`             as `ok`, but wait MS before each answer
//! - `slow-ready:MS`       wait MS before printing ready
//! - `never-ready`         never print ready
//! - `crash-on-start:PATH` append a line to PATH, then exit without printing ready
//! - `hang-once:PATH`      if PATH doesn't exist yet, create it and hang on the first request;
//!                         otherwise behave as `ok`
//! - `exit-once:PATH`      same, but exit instead of hanging
//! - `garbage-once:PATH`   same, but answer the first request with text that isn't JSON
//! - `count:PATH`          as `ok`, appending a line to PATH for every request

use std::io::{BufRead, Write};
use std::time::Duration;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "ok".into());
    let (kind, param) = mode.split_once(':').map_or((mode.as_str(), ""), |(k, p)| (k, p));

    let append = |path: &str| {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        writeln!(f, "x").unwrap();
    };
    let first_run = |path: &str| {
        let fresh = !std::path::Path::new(path).exists();
        if fresh {
            std::fs::write(path, "seen").unwrap();
        }
        fresh
    };

    match kind {
        "crash-on-start" => {
            append(param);
            std::process::exit(1);
        }
        "never-ready" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "slow-ready" => std::thread::sleep(Duration::from_millis(param.parse().unwrap_or(0))),
        _ => {}
    }

    // The sticky misbehaviors apply to the first process only.
    let misbehave = matches!(kind, "hang-once" | "exit-once" | "garbage-once") && first_run(param);

    let mut stdout = std::io::stdout();
    writeln!(stdout, r#"{{"ready":true}}"#).unwrap();
    stdout.flush().unwrap();

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if kind == "count" {
            append(param);
        }
        if misbehave {
            match kind {
                "hang-once" => loop {
                    std::thread::sleep(Duration::from_secs(3600));
                },
                "exit-once" => std::process::exit(0),
                _ => {
                    writeln!(stdout, "this is not json").unwrap();
                    stdout.flush().unwrap();
                    continue;
                }
            }
        }
        if kind == "slow" {
            std::thread::sleep(Duration::from_millis(param.parse().unwrap_or(0)));
        }
        writeln!(stdout, r#"{{"text":"OK"}}"#).unwrap();
        stdout.flush().unwrap();
    }
}
