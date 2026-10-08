//! Word → phonemes (CMU Pronouncing Dictionary) and word → how common it is.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::phoneme::{self, Ph};

static CMUDICT: &str = include_str!("../../data/cmudict.dict");
static WORDFREQ: &str = include_str!("../../data/wordfreq.txt");

pub struct Lexicon {
    pron: HashMap<&'static str, Vec<Ph>>,
    ln_prob: HashMap<&'static str, f64>,
    floor: f64,
}

/// The shared lexicon, parsed on first use.
pub fn get() -> &'static Lexicon {
    static LEX: OnceLock<Lexicon> = OnceLock::new();
    LEX.get_or_init(Lexicon::parse)
}

impl Lexicon {
    fn parse() -> Self {
        let mut pron: HashMap<&'static str, Vec<Ph>> = HashMap::with_capacity(135_000);
        for line in CMUDICT.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut parts = line.split_whitespace();
            let Some(word) = parts.next() else { continue };
            // "word(2)" is an alternate pronunciation; keep the first.
            if word.contains('(') || pron.contains_key(word) {
                continue;
            }
            let phones: Option<Vec<Ph>> = parts.map(phoneme::parse).collect();
            if let Some(p) = phones.filter(|p| !p.is_empty()) {
                pron.insert(word, p);
            }
        }

        let mut counts: Vec<(&'static str, f64)> = Vec::with_capacity(40_000);
        let mut total = 0.0;
        for line in WORDFREQ.lines() {
            if let Some((w, c)) = line.split_once(' ') {
                if let Ok(c) = c.parse::<f64>() {
                    counts.push((w, c));
                    total += c;
                }
            }
        }
        let ln_prob = counts.into_iter().map(|(w, c)| (w, (c / total).ln())).collect();
        // A word outside the 40k list is rarer than anything in it.
        let floor = (1.0 / total).ln() - 1.0;
        Self { pron, ln_prob, floor }
    }

    pub fn phonemes(&self, word: &str) -> Option<&[Ph]> {
        self.pron.get(word.to_lowercase().as_str()).map(Vec::as_slice)
    }

    /// Phonemes for several words in a row; `None` if any word is unknown.
    pub fn phrase<S: AsRef<str>>(&self, words: &[S]) -> Option<Vec<Ph>> {
        let mut out = Vec::new();
        for w in words {
            out.extend_from_slice(self.phonemes(w.as_ref())?);
        }
        (!out.is_empty()).then_some(out)
    }

    /// Natural log of how often a word turns up in English text.
    pub fn ln_prob(&self, word: &str) -> f64 {
        self.ln_prob.get(word.to_lowercase().as_str()).copied().unwrap_or(self.floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ps: &[Ph]) -> Vec<&'static str> {
        ps.iter().map(|&p| phoneme::name(p)).collect()
    }

    #[test]
    fn looks_up_pronunciations() {
        let lex = get();
        assert_eq!(names(lex.phonemes("think").unwrap()), ["TH", "IH", "NG", "K"]);
        assert_eq!(names(lex.phonemes("Water").unwrap()), ["W", "AO", "T", "ER"]);
        assert!(lex.phonemes("zxqvw").is_none());
    }

    #[test]
    fn phrase_needs_every_word() {
        let lex = get();
        assert_eq!(lex.phrase(&["a", "lot"]).map(|p| p.len()), Some(4));
        assert!(lex.phrase(&["a", "zxqvw"]).is_none());
    }

    #[test]
    fn common_words_outrank_rare_ones() {
        let lex = get();
        assert!(lex.ln_prob("thought") > lex.ln_prob("fought"));
        assert!(lex.ln_prob("the") > lex.ln_prob("thought"));
        assert!(lex.ln_prob("zxqvw") < lex.ln_prob("fought"));
    }
}
