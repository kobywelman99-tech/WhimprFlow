//! Passive style learning: a rolling, deterministic profile of the user's
//! natural speaking/writing voice — sentence length, contraction habits, and
//! favorite fillers/phrases — built up from raw transcripts with zero extra
//! LLM calls. [`StyleStore::observe`] updates it after every dictation (no
//! API involved, pure text counting); [`StyleStore::summary_for_prompt`]
//! turns it into a short string folded into the *existing* single cleanup
//! request (see `cleanup::CleanupContext::style_summary`) so cleaned-up text
//! sounds like the user instead of generic AI prose. This adds a little
//! length to that one request's prompt, but never a second request — the
//! whole point was to do this without spending extra tokens/credits.
//!
//! The profile is learned separately per [`StyleMode`] (Email / General /
//! Essay-Report) — see [`resolve_style_mode`] for how a dictation's
//! [`crate::cleanup::FormatMode`] and focused app pick a bucket. People
//! genuinely write differently in an email than in an essay, so blending
//! every dictation into one voice would wash that out.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cleanup::FormatMode;

/// Below this many observed dictations, the profile is too thin to be worth
/// injecting into the prompt — a couple of sentences isn't a "style" yet, and
/// a premature summary would just add noise (and, worse, the model might
/// weakly imitate one atypical utterance as if it were the user's voice).
const MIN_SAMPLES_TO_APPLY: u32 = 5;

/// Common filler/hedge words and discourse markers that signal a casual,
/// conversational voice. Matched as whole words/phrases, case-insensitive.
const FILLERS: &[&str] = &[
    "um", "uh", "like", "you know", "actually", "basically", "honestly",
    "literally", "kind of", "sort of", "i mean", "right", "so anyway",
];

/// Contracted forms and their expanded counterpart, used to measure how often
/// the user's natural speech favors contractions over the fully expanded
/// form. Order doesn't matter; both sides are counted from the same text.
const CONTRACTION_PAIRS: &[(&str, &str)] = &[
    ("don't", "do not"),
    ("doesn't", "does not"),
    ("didn't", "did not"),
    ("can't", "cannot"),
    ("won't", "will not"),
    ("i'm", "i am"),
    ("i've", "i have"),
    ("i'll", "i will"),
    ("i'd", "i would"),
    ("you're", "you are"),
    ("they're", "they are"),
    ("it's", "it is"),
    ("that's", "that is"),
    ("there's", "there is"),
    ("we're", "we are"),
    ("isn't", "is not"),
    ("aren't", "are not"),
    ("wasn't", "was not"),
    ("weren't", "were not"),
];

/// Number of distinct phrases to keep before pruning back down — bounds
/// memory/file size for a profile that otherwise only ever grows.
const MAX_TRACKED_PHRASES: usize = 400;
/// How many to prune back down to once the cap is hit (keeps the highest-count
/// phrases, drops the long tail of one-off n-grams).
const PRUNE_TO: usize = 200;

/// Which learned voice profile a dictation's style signal feeds into. A
/// coarser set than [`FormatMode`] on purpose: `FormatMode::Auto` resolves
/// into one of these via [`resolve_style_mode`], and every auto-detected app
/// category other than email folds into `General` — only an explicit manual
/// `EssayReport` selection populates that bucket, since no app reliably
/// signals "this is an essay."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleMode {
    Email,
    General,
    EssayReport,
}

impl StyleMode {
    pub const ALL: [StyleMode; 3] = [StyleMode::Email, StyleMode::General, StyleMode::EssayReport];
}

/// Resolve which [`StyleMode`] bucket a dictation's style signal should be
/// folded into, from the user's chosen [`FormatMode`] and the focused app
/// (consulted only when the mode is `Auto`).
pub fn resolve_style_mode(format_mode: FormatMode, app_bundle_id: Option<&str>) -> StyleMode {
    match format_mode {
        FormatMode::Email => StyleMode::Email,
        FormatMode::EssayReport => StyleMode::EssayReport,
        FormatMode::General => StyleMode::General,
        FormatMode::Auto => {
            if app_bundle_id.is_some_and(crate::cleanup::prompts::is_email_app) {
                StyleMode::Email
            } else {
                StyleMode::General
            }
        }
    }
}

/// One mode's persisted, ever-growing rolling profile. Every field is a plain
/// counter so `observe` is a cheap, append-only update — the actual summary
/// (averages, top-N lists) is computed on demand in [`summary`]/
/// [`summary_for_prompt`], mirroring how `StatsStore` computes `StatsSummary`
/// fresh from its session log rather than maintaining running averages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StyleProfileData {
    pub samples: u32,
    pub total_words: u64,
    pub total_sentences: u64,
    pub contractions_used: u64,
    pub contractions_expanded: u64,
    #[serde(default)]
    pub filler_counts: HashMap<String, u32>,
    #[serde(default)]
    pub phrase_counts: HashMap<String, u32>,
}

/// What the Hub's Style page shows — a computed, read-only view of the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleSummary {
    pub samples: u32,
    pub min_samples_to_apply: u32,
    /// True once there's enough data that the profile is actually being
    /// applied to cleanup (see `MIN_SAMPLES_TO_APPLY`).
    pub ready: bool,
    pub avg_sentence_words: f32,
    /// 0.0–1.0: how often a contractable phrase appears already contracted.
    pub contraction_rate: f32,
    pub top_fillers: Vec<(String, u32)>,
    pub top_phrases: Vec<(String, u32)>,
    pub tone_label: String,
}

/// The persisted store: one [`StyleProfileData`] per [`StyleMode`], so
/// email/general/essay-report voices are learned independently.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StyleStore {
    #[serde(default)]
    profiles: HashMap<StyleMode, StyleProfileData>,
}

impl StyleStore {
    pub fn load(path: &Path) -> Self {
        let Some(raw) = std::fs::read_to_string(path).ok() else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Self::default();
        };
        // The current shape is `{"profiles": {...}}`. A file written before
        // per-mode profiles existed is the flat counter shape directly (no
        // "profiles" key) — migrate it into the General bucket rather than
        // silently discarding it (an object missing "profiles" would
        // otherwise deserialize into an empty `Self` via `#[serde(default)]`,
        // which would look like a successful load but lose all history).
        if value.get("profiles").is_some() {
            return serde_json::from_value(value).unwrap_or_default();
        }
        match serde_json::from_value::<StyleProfileData>(value) {
            Ok(old) if old.samples > 0 => {
                let mut profiles = HashMap::new();
                profiles.insert(StyleMode::General, old);
                Self { profiles }
            }
            _ => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }

    /// Reset one mode's profile (the Style page's per-mode "Reset" button).
    pub fn reset(&mut self, mode: StyleMode) {
        self.profiles.remove(&mode);
    }

    /// Fold one raw transcript's stylistic signal into `mode`'s profile. Pure
    /// text counting — no network, no LLM, negligible cost.
    pub fn observe(&mut self, mode: StyleMode, raw: &str) {
        self.profiles.entry(mode).or_default().observe(raw);
    }

    /// Compute the display summary for one mode, for the Hub's Style page.
    pub fn summary(&self, mode: StyleMode) -> StyleSummary {
        self.profiles.get(&mode).cloned().unwrap_or_default().summary()
    }

    /// The compact string for one mode, folded into the cleanup prompt.
    pub fn summary_for_prompt(&self, mode: StyleMode) -> Option<String> {
        self.profiles.get(&mode)?.summary_for_prompt()
    }
}

impl StyleProfileData {
    /// Fold one raw transcript's stylistic signal into the rolling profile.
    /// Pure text counting — no network, no LLM, negligible cost.
    pub fn observe(&mut self, raw: &str) {
        let lower = raw.to_lowercase();
        let words: Vec<&str> = raw.split_whitespace().collect();
        if words.is_empty() {
            return;
        }
        self.samples += 1;
        self.total_words += words.len() as u64;
        self.total_sentences += count_sentences(raw).max(1) as u64;

        for (contracted, expanded) in CONTRACTION_PAIRS {
            self.contractions_used += count_occurrences(&lower, contracted) as u64;
            self.contractions_expanded += count_occurrences(&lower, expanded) as u64;
        }

        for filler in FILLERS {
            let n = count_occurrences(&lower, filler);
            if n > 0 {
                *self.filler_counts.entry((*filler).to_string()).or_insert(0) += n as u32;
            }
        }

        for phrase in extract_phrases(&lower) {
            *self.phrase_counts.entry(phrase).or_insert(0) += 1;
        }
        prune(&mut self.phrase_counts);
    }

    /// Compute the display summary for the Hub's Style page.
    pub fn summary(&self) -> StyleSummary {
        let avg_sentence_words = if self.total_sentences == 0 {
            0.0
        } else {
            self.total_words as f32 / self.total_sentences as f32
        };
        let contraction_total = self.contractions_used + self.contractions_expanded;
        let contraction_rate = if contraction_total == 0 {
            0.0
        } else {
            self.contractions_used as f32 / contraction_total as f32
        };
        let top_fillers = top_n(&self.filler_counts, 5, 2);
        // Only surface phrases that recurred — a phrase seen once is just
        // something you happened to say, not part of your style.
        let top_phrases = top_phrases(&self.phrase_counts, 8, 2);
        let tone_label = tone_label(contraction_rate, &top_fillers, self.samples);

        StyleSummary {
            samples: self.samples,
            min_samples_to_apply: MIN_SAMPLES_TO_APPLY,
            ready: self.samples >= MIN_SAMPLES_TO_APPLY,
            avg_sentence_words,
            contraction_rate,
            top_fillers,
            top_phrases,
            tone_label,
        }
    }

    /// The compact string folded into the cleanup prompt, or `None` when
    /// there isn't enough data yet to be worth injecting (see
    /// `MIN_SAMPLES_TO_APPLY`) or the profile has nothing distinctive to say.
    pub fn summary_for_prompt(&self) -> Option<String> {
        let s = self.summary();
        if !s.ready {
            return None;
        }
        let mut parts = Vec::new();
        parts.push(format!("Average sentence length: ~{:.0} words.", s.avg_sentence_words));
        parts.push(format!("Tone: {}.", s.tone_label));
        if s.contraction_rate >= 0.3 {
            parts.push("Uses contractions naturally (don't, it's, I'm) rather than spelling them out.".to_string());
        }
        if !s.top_phrases.is_empty() {
            let phrases: Vec<String> = s.top_phrases.iter().take(5).map(|(p, _)| format!("\"{p}\"")).collect();
            parts.push(format!("Often says things like: {}.", phrases.join(", ")));
        }
        if parts.len() <= 1 {
            return None; // nothing distinctive enough to be worth stating
        }
        Some(parts.join(" "))
    }
}

fn count_sentences(s: &str) -> usize {
    s.chars().filter(|c| matches!(c, '.' | '!' | '?')).count()
}

/// Count non-overlapping whole-word/phrase occurrences of `needle` in
/// `haystack` (both expected lowercase). Boundary-checked so "I'm" doesn't
/// also match inside a longer token.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || h.len() < n.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + n.len() <= h.len() {
        let matches = (0..n.len()).all(|k| h[i + k] == n[k]);
        let boundary_before = i == 0 || !h[i - 1].is_alphanumeric();
        let end = i + n.len();
        let boundary_after = end == h.len() || !h[end].is_alphanumeric();
        if matches && boundary_before && boundary_after {
            count += 1;
            i += n.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Extract 2- and 3-word phrases, skipping ones that are almost entirely
/// stopwords (so "the way that" doesn't crowd out "you know what I mean").
fn extract_phrases(lower: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "of", "to", "in", "on", "is", "it", "for", "with",
    ];
    let toks: Vec<String> = lower
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| c.is_ascii_punctuation()).to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = Vec::new();
    for n in [2, 3] {
        if toks.len() < n {
            continue;
        }
        for w in toks.windows(n) {
            let stop_count = w.iter().filter(|t| STOPWORDS.contains(&t.as_str())).count();
            if stop_count < n {
                out.push(w.join(" "));
            }
        }
    }
    out
}

/// Keep only the top [`PRUNE_TO`] entries once the map exceeds
/// [`MAX_TRACKED_PHRASES`], so a profile that only ever grows doesn't grow
/// without bound.
fn prune(map: &mut HashMap<String, u32>) {
    if map.len() <= MAX_TRACKED_PHRASES {
        return;
    }
    let mut entries: Vec<(String, u32)> = map.drain().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(PRUNE_TO);
    map.extend(entries);
}

/// The top `n` entries by count, excluding anything seen fewer than
/// `min_count` times (a single occurrence isn't a pattern).
fn top_n(map: &HashMap<String, u32>, n: usize, min_count: u32) -> Vec<(String, u32)> {
    let mut entries: Vec<(String, u32)> =
        map.iter().filter(|(_, &c)| c >= min_count).map(|(k, &v)| (k.clone(), v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(n);
    entries
}

/// Like [`top_n`], but without fragments of a longer phrase. Every 2- and
/// 3-word window is counted, so "let me know" also counts as "let me" and
/// "me know"; listing all three says the same thing three times. A fragment is
/// dropped when it rarely appears outside a longer phrase that contains it
/// (fewer than `min_count` uses of its own): "me know" goes, while "thank you"
/// stays if it's often said without "for".
fn top_phrases(map: &HashMap<String, u32>, n: usize, min_count: u32) -> Vec<(String, u32)> {
    let mut candidates: Vec<(String, u32)> =
        map.iter().filter(|(_, &c)| c >= min_count).map(|(k, &v)| (k.clone(), v)).collect();
    // Longest first, so every containing phrase is decided before its fragments.
    candidates.sort_by(|a, b| {
        b.0.split_whitespace().count().cmp(&a.0.split_whitespace().count())
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut kept: Vec<(String, u32)> = Vec::new();
    for (phrase, count) in candidates {
        let inside = format!(" {phrase} ");
        let covered_by = kept
            .iter()
            .filter(|(longer, _)| format!(" {longer} ").contains(&inside))
            .map(|(_, c)| *c)
            .max()
            .unwrap_or(0);
        if count.saturating_sub(covered_by) >= min_count {
            kept.push((phrase, count));
        }
    }
    kept.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    kept.truncate(n);
    kept
}

fn tone_label(contraction_rate: f32, top_fillers: &[(String, u32)], samples: u32) -> String {
    let filler_heavy = top_fillers.iter().map(|(_, c)| *c).sum::<u32>() as f32 / samples.max(1) as f32 > 0.5;
    match (contraction_rate >= 0.4, filler_heavy) {
        (true, true) => "casual and conversational".to_string(),
        (true, false) => "relaxed but direct".to_string(),
        (false, true) => "informal, thinking out loud".to_string(),
        (false, false) => "formal and direct".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_ready_below_min_samples() {
        let mut s = StyleProfileData::default();
        for _ in 0..MIN_SAMPLES_TO_APPLY - 1 {
            s.observe("I don't really know what I'm doing here honestly.");
        }
        assert!(!s.summary().ready);
        assert!(s.summary_for_prompt().is_none());
    }

    #[test]
    fn ready_after_min_samples_with_distinctive_signal() {
        let mut s = StyleProfileData::default();
        for _ in 0..MIN_SAMPLES_TO_APPLY + 2 {
            s.observe("I don't think that's gonna work, you know, it's kind of a mess honestly.");
        }
        let summary = s.summary();
        assert!(summary.ready);
        assert!(summary.contraction_rate > 0.5, "should detect frequent contractions");
        assert!(s.summary_for_prompt().is_some());
    }

    #[test]
    fn formal_expanded_speech_scores_low_contraction_rate() {
        let mut s = StyleProfileData::default();
        for _ in 0..MIN_SAMPLES_TO_APPLY + 1 {
            s.observe("I do not think that is going to work. I will follow up tomorrow.");
        }
        let summary = s.summary();
        assert!(summary.contraction_rate < 0.2);
        assert_eq!(summary.tone_label, "formal and direct");
    }

    #[test]
    fn phrase_counts_are_pruned_when_they_grow_unbounded() {
        let mut s = StyleProfileData::default();
        for i in 0..(MAX_TRACKED_PHRASES + 50) {
            s.observe(&format!("unique phrase number {i} appears only once here today"));
        }
        assert!(s.phrase_counts.len() <= MAX_TRACKED_PHRASES);
    }

    fn temp_style_path() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "whimpr-core-style-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn resolve_style_mode_prefers_manual_override_over_app() {
        assert_eq!(resolve_style_mode(FormatMode::Email, Some("com.notion.app")), StyleMode::Email);
        assert_eq!(resolve_style_mode(FormatMode::General, Some("com.apple.mail")), StyleMode::General);
        assert_eq!(resolve_style_mode(FormatMode::EssayReport, None), StyleMode::EssayReport);
    }

    #[test]
    fn resolve_style_mode_auto_uses_email_app_else_general() {
        assert_eq!(resolve_style_mode(FormatMode::Auto, Some("com.apple.mail")), StyleMode::Email);
        assert_eq!(resolve_style_mode(FormatMode::Auto, Some("com.tinyspeck.slackmacgap")), StyleMode::General);
        assert_eq!(resolve_style_mode(FormatMode::Auto, None), StyleMode::General);
    }

    #[test]
    fn style_store_keeps_modes_independent() {
        let mut store = StyleStore::default();
        for _ in 0..MIN_SAMPLES_TO_APPLY + 1 {
            store.observe(StyleMode::Email, "I'll follow up with you tomorrow, thanks so much.");
        }
        assert!(store.summary(StyleMode::Email).ready);
        assert!(!store.summary(StyleMode::General).ready);
        assert_eq!(store.summary(StyleMode::General).samples, 0);
    }

    #[test]
    fn style_store_reset_only_clears_one_mode() {
        let mut store = StyleStore::default();
        store.observe(StyleMode::Email, "quick note, thanks");
        store.observe(StyleMode::General, "just chatting here");
        store.reset(StyleMode::Email);
        assert_eq!(store.summary(StyleMode::Email).samples, 0);
        assert_eq!(store.summary(StyleMode::General).samples, 1);
    }

    #[test]
    fn style_store_migrates_old_flat_profile_into_general() {
        let path = temp_style_path();
        let mut old = StyleProfileData::default();
        for _ in 0..MIN_SAMPLES_TO_APPLY + 1 {
            old.observe("I don't think that's gonna work, you know, honestly.");
        }
        std::fs::write(&path, serde_json::to_string_pretty(&old).unwrap()).unwrap();

        let migrated = StyleStore::load(&path);
        assert_eq!(migrated.summary(StyleMode::General).samples, old.samples);
        assert!(migrated.summary(StyleMode::General).ready);
        assert_eq!(migrated.summary(StyleMode::Email).samples, 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn style_store_round_trips_through_save_and_load() {
        let path = temp_style_path();
        let mut store = StyleStore::default();
        store.observe(StyleMode::EssayReport, "The committee will convene on Thursday.");
        store.save(&path).unwrap();

        let loaded = StyleStore::load(&path);
        assert_eq!(loaded.summary(StyleMode::EssayReport).samples, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn top_phrases_drop_fragments_of_longer_phrases() {
        let counts: HashMap<String, u32> = [
            ("let me know", 8), ("let me", 10), ("me know", 8), ("know if", 3), ("me know if", 3),
            ("thank you", 12), ("thank you for", 6), ("you for", 6), ("sounds good", 4),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let phrases: Vec<String> = top_phrases(&counts, 8, 2).into_iter().map(|(p, _)| p).collect();
        assert_eq!(phrases, vec!["thank you", "let me", "let me know", "thank you for", "sounds good", "me know if"]);
        // "me know", "you for" and "know if" only ever appear inside a longer phrase.
        // "let me" stays: it was said twice without "know".
    }

    #[test]
    fn top_phrases_match_whole_words_only() {
        // "ship it" isn't inside "worship it": containment is word by word.
        let counts: HashMap<String, u32> =
            [("worship it", 5), ("ship it", 5)].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(top_phrases(&counts, 8, 2).len(), 2);
    }

    #[test]
    fn count_occurrences_respects_word_boundaries() {
        // "I'm" should not match inside a longer token, and should match the
        // standalone word.
        assert_eq!(count_occurrences("i'm going", "i'm"), 1);
        assert_eq!(count_occurrences("gym class", "i'm"), 0);
    }
}
