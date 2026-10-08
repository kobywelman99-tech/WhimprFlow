//! Pure scoring: word error rate and the pass/fail check for cleaned text.

use crate::manifest::{Expect, Fixture};

/// Filler sounds that some engines transcribe and others drop. Removed from both sides
/// of a comparison so an engine isn't scored for its policy on "um".
const FILLERS: &[&str] = &["um", "uh", "umm", "uhh", "erm", "hmm"];

/// Lowercase words without punctuation or fillers. Apostrophes inside a word stay,
/// hyphens and slashes split words, and line breaks are just spaces.
pub fn normalize(text: &str) -> Vec<String> {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' || c == '\u{2019}' { c } else { ' ' })
        .collect();
    cleaned
        .split_whitespace()
        .map(|w| w.trim_matches('\'').replace('\u{2019}', "'").to_lowercase())
        .filter(|w| !w.is_empty() && !FILLERS.contains(&w.as_str()))
        .collect()
}

/// Word error rate: word-level edit distance over the reference length. An empty
/// reference scores 0 when the hypothesis is also empty and 1 otherwise.
pub fn wer(reference: &[String], hypothesis: &[String]) -> f64 {
    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }
    let (n, m) = (reference.len(), hypothesis.len());
    let mut prev: Vec<usize> = (0..=m).collect();
    for i in 1..=n {
        let mut cur = vec![i; m + 1];
        for j in 1..=m {
            let sub = prev[j - 1] + usize::from(reference[i - 1] != hypothesis[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[m] as f64 / n as f64
}

/// Word error rate between two strings, after normalizing both.
pub fn text_wer(reference: &str, hypothesis: &str) -> f64 {
    wer(&normalize(reference), &normalize(hypothesis))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub pass: bool,
    pub reasons: Vec<String>,
}

/// Judge the text that would be pasted. `None` when there is nothing to judge: no
/// cleanup ran (so cleaned-text expectations don't apply) or the fixture has none.
pub fn check_cleaned(fx: &Fixture, got: &str, cleanup_ran: bool) -> Option<Verdict> {
    if fx.expect == Expect::NoOutput {
        let pass = got.trim().is_empty();
        let reasons = if pass { vec![] } else { vec![format!("expected nothing, got {got:?}")] };
        return Some(Verdict { pass, reasons });
    }
    if !cleanup_ran {
        return None;
    }
    let has_checks = fx.expected_cleaned.is_some() || !fx.must_contain.is_empty() || !fx.must_not_contain.is_empty();
    if !has_checks {
        return None;
    }

    let mut reasons = Vec::new();
    if let Some(expected) = &fx.expected_cleaned {
        let w = text_wer(expected, got);
        if w > fx.max_wer {
            reasons.push(format!("WER {w:.2} > {:.2} (expected {expected:?}, got {got:?})", fx.max_wer));
        }
    }
    for needle in &fx.must_contain {
        if !got.contains(needle.as_str()) {
            reasons.push(format!("missing {needle:?}"));
        }
    }
    for needle in &fx.must_not_contain {
        if got.contains(needle.as_str()) {
            reasons.push(format!("must not contain {needle:?}"));
        }
    }
    Some(Verdict { pass: reasons.is_empty(), reasons })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(json: &str) -> Fixture {
        serde_json::from_str(&format!(
            r#"{{"id":"t","wav":"t.wav","category":"c","say":"s",{json}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn normalize_drops_punctuation_case_fillers_and_keeps_apostrophes() {
        assert_eq!(normalize("Um, it's the  Report-Card!\nUh yes."), ["it's", "the", "report", "card", "yes"]);
        assert_eq!(normalize("Don\u{2019}t"), ["don't"]);
        assert!(normalize("  ... ").is_empty());
    }

    #[test]
    fn wer_counts_substitutions_insertions_and_deletions() {
        let r = normalize("the quick brown fox");
        assert_eq!(wer(&r, &normalize("the quick brown fox")), 0.0);
        assert_eq!(wer(&r, &normalize("the quick red fox")), 0.25);
        assert_eq!(wer(&r, &normalize("the quick brown")), 0.25);
        assert_eq!(wer(&r, &normalize("the very quick brown fox")), 0.25);
        assert_eq!(wer(&r, &normalize("")), 1.0);
    }

    #[test]
    fn empty_reference_is_all_or_nothing() {
        assert_eq!(wer(&[], &[]), 0.0);
        assert_eq!(wer(&[], &normalize("thank you")), 1.0);
    }

    #[test]
    fn no_output_clips_pass_only_on_nothing() {
        let f = fixture(r#""expect":"no_output""#);
        assert!(check_cleaned(&f, "", false).unwrap().pass);
        assert!(check_cleaned(&f, "  \n", true).unwrap().pass);
        let v = check_cleaned(&f, "Thank you.", true).unwrap();
        assert!(!v.pass && v.reasons[0].contains("Thank you."));
    }

    #[test]
    fn cleaned_text_is_judged_by_wer_within_the_tolerance() {
        let f = fixture(r#""expected_cleaned":"I think we should ship it on Friday.","max_wer":0.1"#);
        assert!(check_cleaned(&f, "I think we should ship it on Friday.", true).unwrap().pass);
        assert!(check_cleaned(&f, "i think we should ship it on friday", true).unwrap().pass);
        assert!(!check_cleaned(&f, "We should ship it Friday.", true).unwrap().pass);
    }

    #[test]
    fn must_lists_are_exact() {
        let f = fixture(r#""must_contain":["ChargeBee","\n"],"must_not_contain":["Paris"]"#);
        assert!(check_cleaned(&f, "Renew ChargeBee\nnow", true).unwrap().pass);
        let v = check_cleaned(&f, "renew chargebee now Paris", true).unwrap();
        assert_eq!(v.reasons.len(), 3, "{:?}", v.reasons);
    }

    #[test]
    fn nothing_to_judge_is_not_a_pass() {
        let f = fixture(r#""expected_cleaned":"Hello there.""#);
        assert_eq!(check_cleaned(&f, "hello there", false), None, "no cleanup ran");
        assert_eq!(check_cleaned(&fixture(r#""max_wer":0.1"#), "anything", true), None, "no expectations");
    }
}
