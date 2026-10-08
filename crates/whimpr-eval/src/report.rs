//! The result of a run: per-fixture rows, a summary, a table, and a comparison
//! against an earlier report.

use serde::{Deserialize, Serialize};
use whimpr_core::stats::percentile;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    pub resample_ms: u32,
    /// Voice-activity detection (0 when no detector ran).
    #[serde(default)]
    pub vad_ms: u32,
    pub asr_ms: u32,
    /// The cleanup call (0 when it didn't run).
    #[serde(default)]
    pub cleanup_ms: u32,
    /// The sense check (0 when it didn't run).
    #[serde(default)]
    pub review_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Missing,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureResult {
    pub id: String,
    pub category: String,
    pub status: Status,
    #[serde(default)]
    pub error: Option<String>,
    /// Quiet enough that the app would not even run recognition.
    #[serde(default)]
    pub skipped_silent: bool,
    #[serde(default)]
    pub pipeline_ran: bool,
    /// What recognition produced, after mishear fixes.
    #[serde(default)]
    pub raw: String,
    /// What would be pasted.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub raw_wer: Option<f64>,
    #[serde(default)]
    pub cleaned_pass: Option<bool>,
    /// With `--runs N`: how many of the N cleanup runs passed.
    #[serde(default)]
    pub pass_runs: Option<(u32, u32)>,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub approved: bool,
    #[serde(default)]
    pub timings: Timings,
}

impl FixtureResult {
    pub fn missing(id: &str, category: &str) -> Self {
        Self::blank(id, category, Status::Missing)
    }

    pub fn errored(id: &str, category: &str, error: String) -> Self {
        Self { error: Some(error), ..Self::blank(id, category, Status::Error) }
    }

    pub fn blank(id: &str, category: &str, status: Status) -> Self {
        Self {
            id: id.into(),
            category: category.into(),
            status,
            error: None,
            skipped_silent: false,
            pipeline_ran: false,
            raw: String::new(),
            text: String::new(),
            raw_wer: None,
            cleaned_pass: None,
            pass_runs: None,
            reasons: Vec::new(),
            approved: false,
            timings: Timings::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    pub timestamp_unix: u64,
    pub git: Option<String>,
    pub asr: String,
    pub provider: String,
    pub level: String,
    pub review: bool,
    pub runs: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub fixtures: u32,
    pub ran: u32,
    pub missing: u32,
    pub errors: u32,
    pub mean_raw_wer: Option<f64>,
    pub cleaned_checked: u32,
    pub cleaned_passed: u32,
    /// Fixtures whose golden text hasn't been approved yet.
    pub unapproved_goldens: u32,
    pub p50: Timings,
    pub p95: Timings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub meta: Meta,
    pub results: Vec<FixtureResult>,
    pub summary: Summary,
}

impl Report {
    pub fn new(meta: Meta, results: Vec<FixtureResult>) -> Self {
        let summary = summarize(&results);
        Self { meta, results, summary }
    }

    pub fn table(&self) -> String {
        self.render(false)
    }

    /// The table, plus what recognition produced and what would be pasted for each clip.
    pub fn render(&self, show_text: bool) -> String {
        let mut out = format!(
            "{:<30} {:<15} {:>7} {:>8} {:>7} {:>8} {:>9}\n",
            "fixture", "category", "raw WER", "cleaned", "asr ms", "clean ms", "review ms"
        );
        for r in &self.results {
            let wer = r.raw_wer.map_or("-".into(), |w| format!("{w:.2}"));
            let cleaned = match (r.status, r.cleaned_pass, r.skipped_silent) {
                (Status::Missing, ..) => "MISSING".to_string(),
                (Status::Error, ..) => "ERROR".to_string(),
                (_, Some(true), _) => pass_label(r, "PASS"),
                (_, Some(false), _) => pass_label(r, "FAIL"),
                (_, None, _) => "-".to_string(),
            };
            let ms_or_dash = |shown: bool, ms: u32| if shown && ms > 0 { ms.to_string() } else { "-".into() };
            let asr = ms_or_dash(!r.skipped_silent && r.status == Status::Ok, r.timings.asr_ms);
            let clean = ms_or_dash(r.pipeline_ran, r.timings.cleanup_ms);
            let review = ms_or_dash(r.pipeline_ran, r.timings.review_ms);
            out.push_str(&format!(
                "{:<30} {:<15} {:>7} {:>8} {:>7} {:>8} {:>9}\n",
                r.id, r.category, wer, cleaned, asr, clean, review
            ));
            for reason in &r.reasons {
                out.push_str(&format!("    ↳ {reason}\n"));
            }
            if let Some(e) = &r.error {
                out.push_str(&format!("    ↳ {e}\n"));
            }
            if show_text && r.status == Status::Ok {
                out.push_str(&format!("    heard: {:?}\n    paste: {:?}\n", r.raw, r.text));
            }
        }
        let s = &self.summary;
        out.push_str(&format!(
            "\n{} fixtures: {} ran, {} missing, {} errors\n",
            s.fixtures, s.ran, s.missing, s.errors
        ));
        out.push_str(&format!(
            "raw WER (mean): {}\ncleaned text: {}/{} passed{}\n",
            s.mean_raw_wer.map_or("n/a".into(), |w| format!("{w:.3}")),
            s.cleaned_passed,
            s.cleaned_checked,
            if s.unapproved_goldens > 0 { format!(" ({} golden texts not yet approved)", s.unapproved_goldens) } else { String::new() },
        ));
        out.push_str(&format!(
            "latency ms  p50: resample {} vad {} asr {} cleanup {} review {}   p95: resample {} vad {} asr {} cleanup {} review {}\n",
            s.p50.resample_ms,
            s.p50.vad_ms,
            s.p50.asr_ms,
            s.p50.cleanup_ms,
            s.p50.review_ms,
            s.p95.resample_ms,
            s.p95.vad_ms,
            s.p95.asr_ms,
            s.p95.cleanup_ms,
            s.p95.review_ms
        ));
        out
    }
}

fn pass_label(r: &FixtureResult, word: &str) -> String {
    match r.pass_runs {
        Some((k, n)) if n > 1 => format!("{word} {k}/{n}"),
        _ => word.to_string(),
    }
}

fn summarize(results: &[FixtureResult]) -> Summary {
    let ran: Vec<&FixtureResult> = results.iter().filter(|r| r.status == Status::Ok).collect();
    let wers: Vec<f64> = ran.iter().filter_map(|r| r.raw_wer).collect();
    let stage = |p: f64| Timings {
        resample_ms: percentile(&ran.iter().map(|r| r.timings.resample_ms).collect::<Vec<_>>(), p).unwrap_or(0),
        vad_ms: percentile(&ran.iter().map(|r| r.timings.vad_ms).filter(|&ms| ms > 0).collect::<Vec<_>>(), p).unwrap_or(0),
        asr_ms: percentile(
            &ran.iter().filter(|r| !r.skipped_silent).map(|r| r.timings.asr_ms).collect::<Vec<_>>(),
            p,
        )
        .unwrap_or(0),
        // As in the app, a stage that didn't run isn't a fast zero.
        cleanup_ms: percentile(
            &ran.iter().map(|r| r.timings.cleanup_ms).filter(|&ms| ms > 0).collect::<Vec<_>>(),
            p,
        )
        .unwrap_or(0),
        review_ms: percentile(
            &ran.iter().map(|r| r.timings.review_ms).filter(|&ms| ms > 0).collect::<Vec<_>>(),
            p,
        )
        .unwrap_or(0),
    };
    Summary {
        fixtures: results.len() as u32,
        ran: ran.len() as u32,
        missing: results.iter().filter(|r| r.status == Status::Missing).count() as u32,
        errors: results.iter().filter(|r| r.status == Status::Error).count() as u32,
        mean_raw_wer: (!wers.is_empty()).then(|| wers.iter().sum::<f64>() / wers.len() as f64),
        cleaned_checked: ran.iter().filter(|r| r.cleaned_pass.is_some()).count() as u32,
        cleaned_passed: ran.iter().filter(|r| r.cleaned_pass == Some(true)).count() as u32,
        unapproved_goldens: ran.iter().filter(|r| r.cleaned_pass.is_some() && !r.approved).count() as u32,
        p50: stage(50.0),
        p95: stage(95.0),
    }
}

fn delta_ms(label: &str, before: u32, after: u32) -> String {
    format!("  {label}: {before} → {after} ms ({:+})\n", i64::from(after) - i64::from(before))
}

/// What changed from `before` to `after`: totals, latency, and per-fixture regressions.
pub fn compare(before: &Report, after: &Report) -> String {
    let (b, a) = (&before.summary, &after.summary);
    let mut out = String::from("── vs baseline ──\n");
    match (b.mean_raw_wer, a.mean_raw_wer) {
        (Some(x), Some(y)) => out.push_str(&format!("raw WER (mean): {x:.3} → {y:.3} ({:+.3})\n", y - x)),
        _ => out.push_str("raw WER (mean): n/a\n"),
    }
    out.push_str(&format!(
        "cleaned text: {}/{} → {}/{}\n",
        b.cleaned_passed, b.cleaned_checked, a.cleaned_passed, a.cleaned_checked
    ));
    out.push_str("latency p50:\n");
    out.push_str(&delta_ms("resample", b.p50.resample_ms, a.p50.resample_ms));
    out.push_str(&delta_ms("vad", b.p50.vad_ms, a.p50.vad_ms));
    out.push_str(&delta_ms("asr", b.p50.asr_ms, a.p50.asr_ms));
    out.push_str(&delta_ms("cleanup", b.p50.cleanup_ms, a.p50.cleanup_ms));
    out.push_str(&delta_ms("review", b.p50.review_ms, a.p50.review_ms));
    out.push_str("latency p95:\n");
    out.push_str(&delta_ms("resample", b.p95.resample_ms, a.p95.resample_ms));
    out.push_str(&delta_ms("vad", b.p95.vad_ms, a.p95.vad_ms));
    out.push_str(&delta_ms("asr", b.p95.asr_ms, a.p95.asr_ms));
    out.push_str(&delta_ms("cleanup", b.p95.cleanup_ms, a.p95.cleanup_ms));
    out.push_str(&delta_ms("review", b.p95.review_ms, a.p95.review_ms));

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    for now in &after.results {
        let Some(was) = before.results.iter().find(|r| r.id == now.id) else { continue };
        match (was.cleaned_pass, now.cleaned_pass) {
            (Some(true), Some(false)) => regressions.push(format!("{}: cleaned PASS → FAIL", now.id)),
            (Some(false), Some(true)) => improvements.push(format!("{}: cleaned FAIL → PASS", now.id)),
            _ => {}
        }
        if let (Some(x), Some(y)) = (was.raw_wer, now.raw_wer) {
            if y > x + 0.02 {
                regressions.push(format!("{}: raw WER {x:.2} → {y:.2}", now.id));
            } else if y + 0.02 < x {
                improvements.push(format!("{}: raw WER {x:.2} → {y:.2}", now.id));
            }
        }
    }
    for (title, list) in [("regressions", regressions), ("improvements", improvements)] {
        if !list.is_empty() {
            out.push_str(&format!("{title}:\n"));
            for line in list {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(id: &str, wer: Option<f64>, pass: Option<bool>, asr: u32, pipe: u32) -> FixtureResult {
        FixtureResult {
            raw_wer: wer,
            cleaned_pass: pass,
            pipeline_ran: true,
            approved: true,
            timings: Timings { resample_ms: 1, vad_ms: 0, asr_ms: asr, cleanup_ms: pipe, review_ms: 0 },
            ..FixtureResult::blank(id, "c", Status::Ok)
        }
    }

    fn report(results: Vec<FixtureResult>) -> Report {
        Report::new(Meta::default(), results)
    }

    #[test]
    fn the_summary_counts_and_averages() {
        let mut silent = ok("silent", None, Some(true), 0, 0);
        silent.skipped_silent = true;
        silent.pipeline_ran = false;
        let r = report(vec![
            ok("a", Some(0.0), Some(true), 100, 1000),
            ok("b", Some(0.2), Some(false), 300, 2000),
            silent,
            FixtureResult::missing("c", "c"),
            FixtureResult::errored("d", "c", "bad wav".into()),
        ]);
        let s = &r.summary;
        assert_eq!((s.fixtures, s.ran, s.missing, s.errors), (5, 3, 1, 1));
        assert!((s.mean_raw_wer.unwrap() - 0.1).abs() < 1e-9);
        assert_eq!((s.cleaned_checked, s.cleaned_passed, s.unapproved_goldens), (3, 2, 0));
        // Silent clips never reached recognition or the pipeline, so they don't drag latency down.
        assert_eq!((s.p50.asr_ms, s.p95.asr_ms), (100, 300));
        assert_eq!((s.p50.cleanup_ms, s.p95.cleanup_ms), (1000, 2000));
        assert_eq!(s.p50.review_ms, 0, "no review ran");
    }

    #[test]
    fn unapproved_goldens_are_counted() {
        let mut a = ok("a", None, Some(true), 1, 1);
        a.approved = false;
        assert_eq!(report(vec![a]).summary.unapproved_goldens, 1);
    }

    #[test]
    fn the_table_lists_every_fixture_and_the_reasons() {
        let mut bad = ok("bad", Some(0.3), Some(false), 250, 900);
        bad.reasons = vec!["missing \"ChargeBee\"".into()];
        let t = report(vec![ok("good", Some(0.0), Some(true), 120, 800), bad, FixtureResult::missing("gone", "x")]).table();
        for needle in ["good", "PASS", "bad", "FAIL", "↳ missing \"ChargeBee\"", "gone", "MISSING", "1/2 passed"] {
            assert!(t.contains(needle), "{needle} not in\n{t}");
        }
    }

    #[test]
    fn show_text_adds_what_was_heard_and_pasted() {
        let mut a = ok("a", None, None, 1, 1);
        a.raw = "um hello".into();
        a.text = "Hello.".into();
        let r = report(vec![a]);
        assert!(!r.table().contains("heard:"));
        let t = r.render(true);
        assert!(t.contains("heard: \"um hello\"") && t.contains("paste: \"Hello.\""), "{t}");
    }

    #[test]
    fn comparison_reports_deltas_regressions_and_improvements() {
        let before = report(vec![ok("a", Some(0.10), Some(true), 400, 1000), ok("b", Some(0.30), Some(false), 400, 1000)]);
        let after = report(vec![ok("a", Some(0.20), Some(false), 300, 900), ok("b", Some(0.05), Some(true), 300, 900)]);
        let c = compare(&before, &after);
        assert!(c.contains("400 → 300 ms (-100)"), "{c}");
        assert!(c.contains("a: cleaned PASS → FAIL") && c.contains("a: raw WER 0.10 → 0.20"), "{c}");
        assert!(c.contains("b: cleaned FAIL → PASS") && c.contains("b: raw WER 0.30 → 0.05"), "{c}");
    }

    #[test]
    fn reports_survive_a_json_round_trip() {
        let r = report(vec![ok("a", Some(0.1), Some(true), 5, 6)]);
        let back: Report = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.results[0].timings, r.results[0].timings);
        assert_eq!(back.summary.ran, 1);
    }
}
