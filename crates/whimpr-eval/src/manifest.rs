//! The fixture list: which clips exist, what was said, and what must come out.

use serde::{Deserialize, Serialize};
use whimpr_core::dictionary::{DictSource, DictionaryStore};

/// What a clip should produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// Text comes out and is checked against `expected_cleaned` and the must-lists.
    #[default]
    Text,
    /// Silence or noise only: nothing may be pasted.
    NoOutput,
}

fn default_max_wer() -> f64 {
    0.1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    /// File name inside the fixtures directory.
    pub wav: String,
    pub category: String,
    /// What to say when recording this clip (shown in `RECORDING.md`).
    pub say: String,
    /// Extra recording instructions (shown in `RECORDING.md`).
    #[serde(default)]
    pub note: Option<String>,
    /// What the speech recogniser should produce, for accuracy (WER). `null` when the
    /// spoken form and the written form differ too much to compare (numbers, layout cues).
    #[serde(default)]
    pub expected_raw: Option<String>,
    /// The golden cleaned text. `null` when only the must-lists apply.
    #[serde(default)]
    pub expected_cleaned: Option<String>,
    /// Whether the golden text has been reviewed and approved by a person. Checks run
    /// either way; unapproved goldens are counted in the summary.
    #[serde(default)]
    pub approved: bool,
    /// Largest word error rate against `expected_cleaned` that still passes.
    #[serde(default = "default_max_wer")]
    pub max_wer: f64,
    #[serde(default)]
    pub expect: Expect,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub must_not_contain: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryItem {
    pub correct: String,
    #[serde(default)]
    pub mishears: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// The dictionary used for every clip (feeds the whisper prompt and cleanup).
    #[serde(default)]
    pub dictionary: Vec<DictionaryItem>,
    pub fixtures: Vec<Fixture>,
}

impl Manifest {
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let m: Self = serde_json::from_str(json)?;
        let mut seen = std::collections::HashSet::new();
        for f in &m.fixtures {
            anyhow::ensure!(seen.insert(f.id.as_str()), "duplicate fixture id {}", f.id);
        }
        Ok(m)
    }

    pub fn dictionary_store(&self) -> DictionaryStore {
        let mut store = DictionaryStore::default();
        for item in &self.dictionary {
            store.add(item.correct.clone(), item.mishears.clone(), DictSource::Manual);
        }
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_in_the_optional_fields() {
        let m = Manifest::parse(
            r#"{"fixtures":[{"id":"01-a","wav":"01-a.wav","category":"filler","say":"hello there"}]}"#,
        )
        .unwrap();
        let f = &m.fixtures[0];
        assert_eq!((f.expect, f.max_wer, f.approved), (Expect::Text, 0.1, false));
        assert!(f.expected_raw.is_none() && f.expected_cleaned.is_none() && f.must_contain.is_empty());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let one = r#"{"id":"x","wav":"x.wav","category":"c","say":"s"}"#;
        assert!(Manifest::parse(&format!(r#"{{"fixtures":[{one},{one}]}}"#)).is_err());
    }

    #[test]
    fn the_dictionary_becomes_a_store() {
        let m = Manifest::parse(
            r#"{"dictionary":[{"correct":"ChargeBee","mishears":["charge bee"]}],"fixtures":[]}"#,
        )
        .unwrap();
        assert_eq!(m.dictionary_store().apply_replacements("renew charge bee"), "renew ChargeBee");
    }

    #[test]
    fn expect_no_output_parses() {
        let m = Manifest::parse(
            r#"{"fixtures":[{"id":"s","wav":"s.wav","category":"silence","say":"","expect":"no_output"}]}"#,
        )
        .unwrap();
        assert_eq!(m.fixtures[0].expect, Expect::NoOutput);
    }
}
