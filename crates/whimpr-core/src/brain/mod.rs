//! The brain: what WhimprFlow has learned about how this user's speech comes out of
//! the recognizer, with no word list to maintain.
//!
//! It learns from *observations*, a pair of (what the user meant, what the
//! recognizer heard), taken from calibration sessions and from corrections to past
//! dictations. Each observation updates two things:
//!
//! - **Sound patterns** ([`model`]): which phonemes tend to come out as which, e.g.
//!   "my th often arrives as f". This generalises to words never seen before.
//! - **Word memory**: specific "heard X, meant Y" pairs, applied automatically once
//!   they have repeated.
//!
//! At dictation time, [`Brain::rescore`] weighs the recognizer's alternative
//! transcriptions against those patterns, and [`Brain::correct`] applies word memory.

pub mod lexicon;
pub mod model;
pub mod phoneme;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::calibration::align::{diff, segments, Segment};
use crate::dictionary::{DictSource, DictionaryStore};

use self::model::{align, Model, Stats};
use self::phoneme::Ph;

/// How many times a word pair must repeat before it's applied automatically.
const MIN_REPEATS: u32 = 2;
/// An alternative must beat the top result by at least this much (in nats) to win.
const SWITCH_MARGIN: f64 = 0.5;
/// How much each step down the recognizer's ranking counts against an alternative.
const RANK_PENALTY: f64 = 0.2;
/// Alternatives past this rank are ignored.
const MAX_RANK: usize = 5;
/// Most word-memory entries kept; the least-repeated go first.
const MAX_WORDS: usize = 2000;
/// Words this common are never rewritten on their own (as in dictionary calibration).
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "to", "in", "on", "at", "for", "with", "by", "is", "are", "was",
    "were", "be", "been", "it", "its", "i", "you", "he", "she", "we", "they", "me", "my", "your", "his", "her",
    "our", "their", "this", "that", "these", "those", "do", "does", "did", "have", "has", "had", "not", "no",
    "yes", "so", "if", "as", "from", "up", "out", "can", "will", "would", "could", "should", "what", "when",
    "who", "how", "why", "there", "here", "then", "than", "them", "us", "am",
];

/// A specific "the recognizer heard X when I meant Y".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordMemory {
    pub heard: String,
    pub expected: String,
    pub count: u32,
}

/// What is saved to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrainData {
    pub version: u32,
    pub observations: u32,
    pub stats: Stats,
    pub words: Vec<WordMemory>,
    /// Times a remembered `heard` word was heard and was right, so a word that is
    /// usually correct isn't rewritten because of one slip.
    pub heard_right: BTreeMap<String, u32>,
}

/// One sound pattern worth showing the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    /// The sound you meant, e.g. "th (think)".
    pub meant: String,
    /// What it often comes out as, e.g. "f (fan)".
    pub heard: String,
    /// Share of the time this happens, 0–1.
    pub rate: f32,
    /// Times it was observed.
    pub count: u32,
    /// Times that sound came up at all.
    pub opportunities: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub observations: u32,
    /// Word pairs repeated enough to be applied automatically.
    pub learned_words: u32,
    pub patterns: Vec<Pattern>,
}

/// The outcome of choosing among the recognizer's alternatives.
#[derive(Debug, Clone, PartialEq)]
pub struct Rescored {
    pub text: String,
    /// True if an alternative replaced the recognizer's first choice.
    pub switched: bool,
}

#[derive(Debug, Clone)]
pub struct Brain {
    data: BrainData,
    model: Model,
}

impl Default for Brain {
    fn default() -> Self {
        Self::from_data(BrainData { version: 1, ..BrainData::default() })
    }
}

/// How different two spoken phrases sound: 0 for the same, 1 for nothing alike.
/// `None` if a word isn't in the pronunciation dictionary.
pub fn phonetic_gap<S: AsRef<str>>(a: &[S], b: &[S]) -> Option<f64> {
    let lex = lexicon::get();
    let (pa, pb) = (lex.phrase(a)?, lex.phrase(b)?);
    let cost: f64 = align(&pa, &pb)
        .iter()
        .map(|e| match *e {
            model::Edit::Match(_) => 0.0,
            model::Edit::Sub(x, y) => phoneme::distance(x, y).min(1.0),
            model::Edit::Del(_) | model::Edit::Ins(_) => 1.0,
        })
        .sum();
    Some((cost / pa.len().max(pb.len()) as f64).min(1.0))
}

fn is_stop(word: &str) -> bool {
    STOP_WORDS.contains(&word)
}

impl Brain {
    fn from_data(data: BrainData) -> Self {
        let model = Model::from_stats(&data.stats);
        Self { data, model }
    }

    /// Load from `path`; a missing or unreadable file gives an empty brain.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<BrainData>(&s).ok())
            .map(Self::from_data)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string(&self.data).unwrap_or_default())
    }

    /// Forget everything.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn observations(&self) -> u32 {
        self.data.observations
    }

    /// Learn from one utterance: what the user `meant` against what the recognizer `heard`.
    pub fn observe(&mut self, meant: &str, heard: &str) {
        let lex = lexicon::get();
        let remembered: Vec<String> = self.data.words.iter().map(|w| w.heard.clone()).collect();

        // Sound patterns: every aligned word pair teaches the model, hits included.
        for seg in segments(meant, heard) {
            match seg {
                Segment::Same(word) => {
                    if let Some(ph) = lex.phonemes(&word) {
                        self.data.stats.record(&align(ph, ph));
                    }
                    let lower = word.to_lowercase();
                    if remembered.contains(&lower) {
                        *self.data.heard_right.entry(lower).or_default() += 1;
                    }
                }
                Segment::Differ { expected, heard, .. } => {
                    // Short, plausible runs only; a long garble says nothing about sounds.
                    if (1..=3).contains(&expected.len()) && (1..=3).contains(&heard.len()) {
                        if let (Some(e), Some(h)) = (lex.phrase(&expected), lex.phrase(&heard)) {
                            self.data.stats.record(&align(&e, &h));
                        }
                    }
                }
            }
        }

        // Word memory: the specific pairs.
        for m in diff(meant, heard) {
            if !m.heard.contains(' ') && is_stop(&m.heard) {
                continue;
            }
            match self.data.words.iter_mut().find(|w| w.heard == m.heard && w.expected.eq_ignore_ascii_case(&m.expected)) {
                Some(w) => w.count += 1,
                None => self.data.words.push(WordMemory { heard: m.heard, expected: m.expected, count: 1 }),
            }
        }
        if self.data.words.len() > MAX_WORDS {
            self.data.words.sort_by_key(|w| std::cmp::Reverse(w.count));
            self.data.words.truncate(MAX_WORDS);
        }

        self.data.observations += 1;
        self.model = Model::from_stats(&self.data.stats);
    }

    /// Word pairs strong enough to apply: repeated, and the heard word is
    /// wrong more often than right.
    fn active_words(&self) -> impl Iterator<Item = &WordMemory> {
        self.data.words.iter().filter(|w| {
            w.count >= MIN_REPEATS && w.count > 2 * self.data.heard_right.get(&w.heard).copied().unwrap_or(0)
        })
    }

    /// Rewrite remembered mishears (whole words) to what they were meant to be.
    pub fn correct(&self, text: &str) -> String {
        let mut rules = DictionaryStore::default();
        for w in self.active_words() {
            rules.add(w.expected.clone(), vec![w.heard.clone()], DictSource::Auto);
        }
        if rules.entries.is_empty() {
            return text.to_string();
        }
        rules.apply_replacements(text)
    }

    /// Choose among the recognizer's candidate transcriptions (best first).
    ///
    /// For each alternative this asks: if the user meant *this*, how likely is it
    /// that their speech came out as the recognizer's first choice, given their sound
    /// patterns, and how common are the words? The alternative replaces the first
    /// choice only if it clearly wins. With no learned patterns, or with words the
    /// pronunciation dictionary doesn't know, the first choice stands.
    pub fn rescore(&self, candidates: &[String]) -> Rescored {
        let Some(top) = candidates.first() else {
            return Rescored { text: String::new(), switched: false };
        };
        let keep = Rescored { text: top.clone(), switched: false };
        if self.data.observations == 0 {
            return keep;
        }

        let lex = lexicon::get();
        let mut best: Option<(f64, &String)> = None;
        for (rank, alt) in candidates.iter().enumerate().skip(1).take(MAX_RANK) {
            let Some(delta) = self.advantage(lex, alt, top) else { continue };
            let score = delta - RANK_PENALTY * rank as f64;
            if score > SWITCH_MARGIN && best.is_none_or(|(b, _)| score > b) {
                best = Some((score, alt));
            }
        }
        match best {
            Some((_, alt)) => Rescored { text: alt.clone(), switched: true },
            None => keep,
        }
    }

    /// How much more plausible `alt` is than `top` as what the user meant, given
    /// that the recognizer produced `top`. `None` if they can't be compared.
    fn advantage(&self, lex: &lexicon::Lexicon, alt: &str, top: &str) -> Option<f64> {
        let mut total = 0.0;
        let mut compared = 0;
        for seg in segments(alt, top) {
            let Segment::Differ { expected, heard, .. } = seg else { continue };
            if expected.is_empty() || heard.is_empty() || expected.len() > 3 || heard.len() > 3 {
                return None;
            }
            let (alt_ph, top_ph) = (lex.phrase(&expected)?, lex.phrase(&heard)?);
            let alt_score = self.model.ln_likelihood(&alt_ph, &top_ph) + expected.iter().map(|w| lex.ln_prob(w)).sum::<f64>();
            let top_score = self.model.ln_likelihood(&top_ph, &top_ph) + heard.iter().map(|w| lex.ln_prob(w)).sum::<f64>();
            total += alt_score - top_score;
            compared += 1;
        }
        (compared > 0 && compared <= 2).then_some(total)
    }

    /// Spellings the recognizer should lean toward: words the user has been
    /// misheard saying, most repeated first.
    pub fn bias_words(&self, limit: usize) -> Vec<String> {
        let mut words: Vec<&WordMemory> = self.data.words.iter().collect();
        words.sort_by_key(|w| std::cmp::Reverse(w.count));
        let mut out: Vec<String> = Vec::new();
        for w in words {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&w.expected)) {
                out.push(w.expected.clone());
            }
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    /// What the brain has noticed, for the screen that shows it.
    pub fn summary(&self) -> Summary {
        let stats = &self.data.stats;
        let mut patterns = Vec::new();
        for e in 0..phoneme::COUNT as Ph {
            let opps = stats.opportunities_of(e);
            let subs = stats.substitutions_of(e);
            if opps < 5 || subs < 2 {
                continue;
            }
            let rate = subs as f32 / opps as f32;
            if rate < 0.15 {
                continue;
            }
            let top = (0..phoneme::COUNT as Ph).filter(|&h| h != e).max_by_key(|&h| stats.substitution_count(e, h));
            if let Some(h) = top {
                patterns.push(Pattern {
                    meant: phoneme::label(e).to_string(),
                    heard: phoneme::label(h).to_string(),
                    rate: stats.substitution_count(e, h) as f32 / opps as f32,
                    count: stats.substitution_count(e, h),
                    opportunities: opps,
                });
            }
        }
        patterns.sort_by(|a, b| (b.rate * b.count as f32).total_cmp(&(a.rate * a.count as f32)));
        patterns.truncate(8);
        Summary {
            observations: self.data.observations,
            learned_words: self.active_words().count() as u32,
            patterns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A brain that has heard "th" come out as "f" in about half of its chances.
    fn f_for_th_brain() -> Brain {
        let mut b = Brain::default();
        for _ in 0..4 {
            b.observe("thought", "fought");
            b.observe("three", "free");
            b.observe("thin", "fin");
            b.observe("think about it", "think about it");
            b.observe("thank you for the thing", "thank you for the thing");
        }
        b
    }

    fn cands(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_fresh_brain_trusts_the_recognizer() {
        let b = Brain::default();
        assert_eq!(b.rescore(&cands(&["he fought it", "he thought it"])).text, "he fought it");
        assert_eq!(b.correct("anything at all"), "anything at all");
    }

    #[test]
    fn an_untrained_pattern_does_not_flip_a_real_word() {
        // Observed plenty of speech, but no th→f: "fought" is a fine word.
        let mut b = Brain::default();
        for _ in 0..10 {
            b.observe("see the red car", "see the red car");
        }
        assert!(!b.rescore(&cands(&["he fought it", "he thought it"])).switched);
    }

    #[test]
    fn learning_th_for_f_prefers_the_common_word() {
        let r = f_for_th_brain().rescore(&cands(&["he fought it", "he thought it"]));
        assert!(r.switched);
        assert_eq!(r.text, "he thought it");
    }

    #[test]
    fn the_pattern_generalises_to_words_never_observed() {
        // Trained on three/free, thin/fin and thread/fred only: never on thought/fought.
        let mut b = Brain::default();
        for _ in 0..4 {
            b.observe("three", "free");
            b.observe("thin", "fin");
            b.observe("thread", "fred");
            b.observe("think about it", "think about it");
            b.observe("thank you for the thing", "thank you for the thing");
        }
        assert!(b.data.words.iter().all(|w| w.expected != "thought"));
        let r = b.rescore(&cands(&["he fought it", "he thought it"]));
        assert!(r.switched, "th→f learned from other words should carry over");
        assert_eq!(r.text, "he thought it");
        // And it doesn't invent th where the sounds aren't related.
        assert!(!b.rescore(&cands(&["a cat", "a dog"])).switched);
    }

    #[test]
    fn unrelated_alternatives_are_left_alone() {
        let r = f_for_th_brain().rescore(&cands(&["send it now", "sent it now"]));
        assert!(!r.switched);
    }

    #[test]
    fn repeated_word_pairs_are_applied() {
        let mut b = Brain::default();
        b.observe("water please", "wooder please");
        assert_eq!(b.correct("a wooder"), "a wooder", "once is not a pattern");
        b.observe("cold water", "cold wooder");
        assert_eq!(b.correct("a Wooder"), "a Water");
    }

    #[test]
    fn a_word_that_is_usually_right_is_not_rewritten() {
        let mut b = Brain::default();
        b.observe("a ledge", "a hedge");
        b.observe("a ledge", "a hedge");
        for _ in 0..5 {
            b.observe("the hedge row", "the hedge row");
        }
        assert_eq!(b.correct("a hedge"), "a hedge");
    }

    #[test]
    fn common_function_words_are_never_remembered() {
        let mut b = Brain::default();
        for _ in 0..5 {
            b.observe("want the red one", "want a red one");
        }
        assert_eq!(b.correct("a red one"), "a red one");
    }

    #[test]
    fn summary_names_the_pattern() {
        let s = f_for_th_brain().summary();
        assert_eq!(s.observations, 20);
        let top = s.patterns.first().expect("a th pattern");
        assert!(top.meant.starts_with("th"), "{top:?}");
        assert!(top.heard.starts_with("f"), "{top:?}");
        assert!(top.rate > 0.3 && top.rate <= 1.0);
    }

    #[test]
    fn bias_words_lists_what_was_misheard() {
        let b = f_for_th_brain();
        let w = b.bias_words(10);
        assert!(w.iter().any(|w| w == "thought"));
    }

    #[test]
    fn saves_and_loads_without_loss() {
        let dir = std::env::temp_dir().join(format!("brain-{}", std::process::id()));
        let path = dir.join("brain.json");
        let b = f_for_th_brain();
        b.save(&path).unwrap();
        let loaded = Brain::load(&path);
        assert_eq!(loaded.observations(), b.observations());
        assert_eq!(loaded.summary(), b.summary());
        assert_eq!(
            loaded.rescore(&cands(&["he fought it", "he thought it"])),
            b.rescore(&cands(&["he fought it", "he thought it"]))
        );
        assert_eq!(Brain::load(&dir.join("missing.json")).observations(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn phonetic_gap_tells_near_from_far() {
        let near = phonetic_gap(&["rapport"], &["report"]).unwrap();
        let far = phonetic_gap(&["cat"], &["president"]).unwrap();
        assert!(near < 0.5, "{near}");
        assert!(far > 0.7, "{far}");
        assert_eq!(phonetic_gap(&["water"], &["water"]), Some(0.0));
        assert_eq!(phonetic_gap(&["zxqvw"], &["water"]), None);
    }

    #[test]
    fn reset_forgets() {
        let mut b = f_for_th_brain();
        b.reset();
        assert_eq!(b.observations(), 0);
        assert!(b.summary().patterns.is_empty());
    }
}
