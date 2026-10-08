//! Custom dictionary: user vocabulary plus a pre-filter that injects only the
//! entries relevant to a given utterance into the cleanup prompt (fewer distractors
//! → higher LLM precision). Manual entries and auto-learned (✨) entries share the
//! same store; the auto-learn diff engine (needs accessibility reads) layers on top.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cleanup::VocabEntry;

/// How a dictionary entry was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictSource {
    Manual,
    Auto,
}

fn default_source() -> DictSource {
    DictSource::Manual
}

/// One vocabulary entry: the authoritative spelling and known mishears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub correct: String,
    #[serde(default)]
    pub mishears: Vec<String>,
    #[serde(default = "default_source")]
    pub source: DictSource,
}

/// The user's dictionary, persisted as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DictionaryStore {
    pub entries: Vec<DictionaryEntry>,
}

impl DictionaryStore {
    /// Load from `path`, returning an empty store if missing or unreadable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist to `path` (creating parent dirs).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        std::fs::write(path, json)
    }

    /// Add or merge an entry, de-duplicating by spelling (case-insensitive).
    pub fn add(&mut self, correct: impl Into<String>, mishears: Vec<String>, source: DictSource) {
        let correct = correct.into();
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.correct.eq_ignore_ascii_case(&correct))
        {
            for m in mishears {
                if !existing.mishears.iter().any(|x| x.eq_ignore_ascii_case(&m)) {
                    existing.mishears.push(m);
                }
            }
        } else {
            self.entries.push(DictionaryEntry {
                correct,
                mishears,
                source,
            });
        }
    }

    /// Remove an entry by its spelling (case-insensitive). Returns true if removed.
    pub fn remove(&mut self, correct: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| !e.correct.eq_ignore_ascii_case(correct));
        self.entries.len() != before
    }

    /// Deterministically rewrite known mishears to their correct spelling.
    /// Whole-word (and whole-phrase) matches only, case-insensitive; the source
    /// text's capitalisation style (ALL CAPS / Capitalised) carries over to
    /// `correct` unless `correct` has its own mixed case (e.g. "ChargeBee").
    pub fn apply_replacements(&self, text: &str) -> String {
        let mut rules: Vec<(Vec<String>, &str)> = Vec::new();
        for e in &self.entries {
            for m in &e.mishears {
                let words: Vec<String> = word_spans(m).iter().map(|&(a, b)| m[a..b].to_lowercase()).collect();
                if words.is_empty() || words.join(" ") == e.correct.to_lowercase() {
                    continue;
                }
                rules.push((words, e.correct.as_str()));
            }
        }
        if rules.is_empty() {
            return text.to_string();
        }
        rules.sort_by_key(|r| std::cmp::Reverse(r.0.len()));

        let spans = word_spans(text);
        let lower: Vec<String> = spans.iter().map(|&(a, b)| text[a..b].to_lowercase()).collect();
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        let mut i = 0;
        while i < spans.len() {
            let hit = rules.iter().find(|(w, _)| {
                i + w.len() <= spans.len()
                    && lower[i..i + w.len()] == w[..]
                    && (i..i + w.len() - 1).all(|k| {
                        text[spans[k].1..spans[k + 1].0].chars().all(char::is_whitespace)
                    })
            });
            match hit {
                Some((w, correct)) => {
                    let (start, _) = spans[i];
                    let (_, end) = spans[i + w.len() - 1];
                    out.push_str(&text[pos..start]);
                    out.push_str(&match_case(&text[start..end], correct));
                    pos = end;
                    i += w.len();
                }
                None => i += 1,
            }
        }
        out.push_str(&text[pos..]);
        out
    }

    /// Spellings plus mishears worth biasing the recognizer toward: correct
    /// spellings only (a mishear in the context list would reinforce the error).
    pub fn bias_words(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.correct.clone()).collect()
    }

    /// Select the entries relevant to `utterance` — those whose spelling or a known
    /// mishear is edit-close to a spoken token (or adjacent token pair, to catch
    /// split words like "charge bee" → "ChargeBee") — capped to `max`.
    pub fn prefilter(&self, utterance: &str, max: usize) -> Vec<VocabEntry> {
        let toks: Vec<String> = utterance
            .split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| c.is_ascii_punctuation())
                    .to_lowercase()
            })
            .filter(|t| !t.is_empty())
            .collect();

        let mut grams: Vec<String> = toks.clone();
        for w in toks.windows(2) {
            grams.push(format!("{}{}", w[0], w[1]));
        }

        let mut out = Vec::new();
        for e in &self.entries {
            let targets: Vec<String> = std::iter::once(e.correct.to_lowercase())
                .chain(e.mishears.iter().map(|m| m.to_lowercase()))
                .collect();
            if grams.iter().any(|g| targets.iter().any(|t| close(g, t))) {
                out.push(VocabEntry {
                    correct: e.correct.clone(),
                    mishears: e.mishears.clone(),
                });
                if out.len() >= max {
                    break;
                }
            }
        }
        out
    }
}

/// Byte ranges of the words (alphanumeric runs, with inner apostrophes) in `s`.
fn word_spans(s: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (idx, &(b, c)) in chars.iter().enumerate() {
        let inner_apos = (c == '\'' || c == '\u{2019}')
            && start.is_some()
            && chars.get(idx + 1).is_some_and(|&(_, n)| n.is_alphanumeric());
        if c.is_alphanumeric() || inner_apos {
            start.get_or_insert(b);
        } else if let Some(a) = start.take() {
            spans.push((a, b));
        }
    }
    if let Some(a) = start {
        spans.push((a, s.len()));
    }
    spans
}

fn match_case(src: &str, correct: &str) -> String {
    let has_inner_upper = correct.chars().skip(1).any(char::is_uppercase);
    let letters: Vec<char> = src.chars().filter(|c| c.is_alphabetic()).collect();
    if has_inner_upper || correct.chars().next().is_some_and(char::is_uppercase) {
        return correct.to_string();
    }
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        correct.to_uppercase()
    } else if letters.first().is_some_and(|c| c.is_uppercase()) {
        let mut cs = correct.chars();
        cs.next().map(|f| f.to_uppercase().chain(cs).collect()).unwrap_or_default()
    } else {
        correct.to_string()
    }
}

/// Two tokens are "close" if identical or within a normalized edit distance of 0.34.
fn close(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let maxlen = a.chars().count().max(b.chars().count());
    if maxlen == 0 {
        return false;
    }
    (strsim::levenshtein(a, b) as f32 / maxlen as f32) <= 0.34
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> DictionaryStore {
        let mut s = DictionaryStore::default();
        s.add("Manvi", vec!["Monvi".into(), "Manvee".into()], DictSource::Manual);
        s.add("ChargeBee", vec!["charge bee".into()], DictSource::Manual);
        s
    }

    #[test]
    fn prefilter_selects_close_mishear() {
        // "monvi" is an exact mishear of Manvi.
        let v = store().prefilter("send the deck to monvi please", 15);
        assert!(v.iter().any(|e| e.correct == "Manvi"));
        assert!(!v.iter().any(|e| e.correct == "ChargeBee"));
    }

    #[test]
    fn prefilter_catches_split_word_via_bigram() {
        // "charge bee" spoken as two words → bigram "chargebee" matches.
        let v = store().prefilter("we should renew charge bee this month", 15);
        assert!(v.iter().any(|e| e.correct == "ChargeBee"));
    }

    #[test]
    fn prefilter_ignores_unrelated_utterance() {
        let v = store().prefilter("the weather is nice today", 15);
        assert!(v.is_empty());
    }

    #[test]
    fn replacements_are_whole_word_and_case_aware() {
        let mut s = DictionaryStore::default();
        s.add("Manvi", vec!["Monvi".into()], DictSource::Manual);
        s.add("ChargeBee", vec!["charge bee".into()], DictSource::Manual);
        s.add("water", vec!["wooder".into()], DictSource::Manual);
        assert_eq!(s.apply_replacements("tell monvi, ok"), "tell Manvi, ok");
        assert_eq!(s.apply_replacements("renew Charge  Bee now"), "renew ChargeBee now");
        assert_eq!(s.apply_replacements("Wooder is wet"), "Water is wet");
        assert_eq!(s.apply_replacements("WOODER"), "WATER");
        assert_eq!(s.apply_replacements("monvis car"), "monvis car");
        assert_eq!(s.apply_replacements("nothing here"), "nothing here");
    }

    #[test]
    fn legacy_json_without_new_source_still_loads() {
        let j = r#"{"entries":[{"correct":"A","mishears":["b"]}]}"#;
        let s: DictionaryStore = serde_json::from_str(j).unwrap();
        assert_eq!(s.entries[0].source, DictSource::Manual);
    }

    #[test]
    fn add_merges_mishears_case_insensitively() {
        let mut s = store();
        s.add("manvi", vec!["Manvie".into()], DictSource::Auto);
        let e = s.entries.iter().find(|e| e.correct == "Manvi").unwrap();
        assert!(e.mishears.iter().any(|m| m == "Manvie"));
        assert_eq!(s.entries.iter().filter(|e| e.correct.eq_ignore_ascii_case("manvi")).count(), 1);
    }
}
