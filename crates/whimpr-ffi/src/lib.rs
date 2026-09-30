//! Swift bindings for the iOS app.
//!
//! Cleanup on iOS runs the desktop pipeline (`src-tauri/src/hotkey.rs`,
//! `clean_transcript`) split around the network call, which the app makes
//! itself with URLSession:
//!
//! 1. [`prepare_cleanup`]: layout cues → markers, the prompt messages, and the
//!    text to fall back to.
//! 2. The app sends the messages to the provider.
//! 3. [`finish_cleanup`]: post-processing and the deterministic gates. On a
//!    failed gate (or if step 2 failed) the app uses the fallback.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use whimpr_core::cleanup::{self, CleanupContext, CleanupLevel, VocabEntry};
use whimpr_core::dictionary::{DictSource, DictionaryStore};
use whimpr_core::style::StyleStore;

uniffi::setup_scaffolding!();

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Level {
    /// Paste exactly what was said; no model call. (CleanupLevel::None; not
    /// called `None` because Swift would read `.none` as a nil Optional.)
    Off,
    Light,
    Medium,
    High,
}

impl From<Level> for CleanupLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::Off => CleanupLevel::None,
            Level::Light => CleanupLevel::Light,
            Level::Medium => CleanupLevel::Medium,
            Level::High => CleanupLevel::High,
        }
    }
}

/// The default level, from the core (Light).
#[uniffi::export]
pub fn default_level() -> Level {
    match CleanupLevel::default() {
        CleanupLevel::None => Level::Off,
        CleanupLevel::Light => Level::Light,
        CleanupLevel::Medium => Level::Medium,
        CleanupLevel::High => Level::High,
    }
}

/// Which learned voice profile a style call reads/writes — mirrors
/// `whimpr_core::style::StyleMode`. The desktop Hub also lets a dictation's
/// mode be auto-detected from the focused app; iOS currently always passes
/// `General` (see `StyleModel.swift`) until it has its own mode picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StyleMode {
    Email,
    General,
    EssayReport,
}

impl From<StyleMode> for whimpr_core::style::StyleMode {
    fn from(mode: StyleMode) -> Self {
        match mode {
            StyleMode::Email => whimpr_core::style::StyleMode::Email,
            StyleMode::General => whimpr_core::style::StyleMode::General,
            StyleMode::EssayReport => whimpr_core::style::StyleMode::EssayReport,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Vocab {
    pub correct: String,
    pub mishears: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChatMessage {
    /// "user" or "assistant". The system prompt is separate.
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CleanupRequest {
    /// Don't call a model; insert `fallback` as is (level None, or nothing said).
    pub skip_model: bool,
    pub system: String,
    /// Few-shot turns, then the transcript. Anthropic takes these as `messages`
    /// with `system` alongside; OpenAI wants `system` as the first message.
    pub messages: Vec<ChatMessage>,
    /// What to insert if the model fails or its answer is rejected.
    pub fallback: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CleanupOutcome {
    pub text: String,
    /// False when the gates rejected the model's text and `text` is the fallback.
    pub accepted: bool,
    /// Why it was rejected, for logs.
    pub rejection: Option<String>,
}

#[uniffi::export]
pub fn prepare_cleanup(
    raw: String,
    level: Level,
    vocab: Vec<Vocab>,
    app_bundle_id: Option<String>,
    style_summary: Option<String>,
) -> CleanupRequest {
    let level = CleanupLevel::from(level);
    if level.bypasses_llm() || raw.trim().is_empty() {
        return CleanupRequest {
            skip_model: true,
            system: String::new(),
            messages: Vec::new(),
            fallback: raw,
        };
    }
    // As on desktop: the model sees the cue markers; the fallback and the gate
    // see them restored to real line breaks.
    let raw_norm = cleanup::pre_normalize_layout(&raw);
    let fallback = cleanup::post_process(&raw_norm);
    let ctx = CleanupContext {
        level,
        vocab: vocab
            .into_iter()
            .map(|v| VocabEntry { correct: v.correct, mishears: v.mishears })
            .collect(),
        app_bundle_id,
        style_summary,
        ..Default::default()
    };

    let mut system = String::new();
    let mut messages = Vec::new();
    for message in cleanup::build_messages(&raw_norm, &ctx) {
        if message.role == "system" {
            system = message.content;
        } else {
            messages.push(ChatMessage { role: message.role.to_string(), content: message.content });
        }
    }
    CleanupRequest { skip_model: false, system, messages, fallback }
}

#[uniffi::export]
pub fn finish_cleanup(model_output: String, fallback: String, level: Level) -> CleanupOutcome {
    let cleaned = cleanup::post_process(&model_output);
    if cleaned.trim().is_empty() {
        return CleanupOutcome { text: fallback, accepted: false, rejection: Some("empty output".into()) };
    }
    match cleanup::evaluate_gates(&fallback, &cleaned, level.into()) {
        cleanup::GateVerdict::Pass => CleanupOutcome { text: cleaned, accepted: true, rejection: None },
        cleanup::GateVerdict::Fail(reason) => CleanupOutcome {
            text: fallback,
            accepted: false,
            rejection: Some(format!("{reason:?}")),
        },
    }
}

// MARK: Dictionary

/// Entries passed to the cleanup prompt per dictation; same cap as desktop.
pub const MAX_RELEVANT_ENTRIES: u32 = 15;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DictEntry {
    pub correct: String,
    pub mishears: Vec<String>,
    /// Added automatically from a correction, rather than typed in.
    pub learned: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum DictionaryError {
    #[error("Enter a word to add.")]
    EmptyWord,
    #[error("Couldn't save the dictionary: {message}")]
    Io { message: String },
}

/// The user's dictionary: whimpr-core's `DictionaryStore`, saved to `path`
/// after every change in the same JSON format as the desktop's
/// dictionary.json.
#[derive(uniffi::Object)]
pub struct UserDictionary {
    path: PathBuf,
    store: Mutex<DictionaryStore>,
}

#[uniffi::export]
impl UserDictionary {
    /// Opens the dictionary at `path`; a missing or unreadable file is empty.
    #[uniffi::constructor]
    pub fn open(path: String) -> Arc<Self> {
        let path = PathBuf::from(path);
        let store = DictionaryStore::load(&path);
        Arc::new(Self { path, store: Mutex::new(store) })
    }

    pub fn entries(&self) -> Vec<DictEntry> {
        self.lock()
            .entries
            .iter()
            .map(|e| DictEntry {
                correct: e.correct.clone(),
                mishears: e.mishears.clone(),
                learned: e.source == DictSource::Auto,
            })
            .collect()
    }

    /// Adds a word, or merges new mishears into an existing one (matched
    /// ignoring case), then saves.
    pub fn add(&self, correct: String, mishears: Vec<String>) -> Result<(), DictionaryError> {
        let correct = correct.trim();
        if correct.is_empty() {
            return Err(DictionaryError::EmptyWord);
        }
        let mishears = mishears
            .iter()
            .map(|m| m.trim())
            .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case(correct))
            .map(String::from)
            .collect();
        let mut store = self.lock();
        store.add(correct, mishears, DictSource::Manual);
        self.save(&store)
    }

    /// Removes a word (ignoring case) and saves. Returns false if it wasn't there.
    pub fn remove(&self, correct: String) -> Result<bool, DictionaryError> {
        let mut store = self.lock();
        let removed = store.remove(correct.trim());
        if removed {
            self.save(&store)?;
        }
        Ok(removed)
    }

    /// The entries that sound close to something in `utterance`, for the
    /// cleanup prompt (whimpr-core's prefilter, as on desktop).
    pub fn relevant(&self, utterance: String) -> Vec<Vocab> {
        self.lock()
            .prefilter(&utterance, MAX_RELEVANT_ENTRIES as usize)
            .into_iter()
            .map(|v| Vocab { correct: v.correct, mishears: v.mishears })
            .collect()
    }

    /// Every correct spelling, for the speech recogniser's vocabulary hints.
    pub fn words(&self) -> Vec<String> {
        self.lock().entries.iter().map(|e| e.correct.clone()).collect()
    }
}

impl UserDictionary {
    fn lock(&self) -> MutexGuard<'_, DictionaryStore> {
        // A panic mid-edit can't leave the store half-written, so keep going.
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, store: &DictionaryStore) -> Result<(), DictionaryError> {
        store.save(&self.path).map_err(|e| DictionaryError::Io { message: e.to_string() })
    }
}

// MARK: Style

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PhraseCount {
    pub phrase: String,
    pub count: u32,
}

/// What the Style screen shows (whimpr-core's `StyleSummary`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StyleOverview {
    pub samples: u32,
    pub min_samples_to_apply: u32,
    /// Enough samples that the profile shapes cleanup.
    pub ready: bool,
    pub avg_sentence_words: f32,
    /// 0–1: how often a contractable phrase comes out contracted.
    pub contraction_rate: f32,
    pub top_fillers: Vec<PhraseCount>,
    pub top_phrases: Vec<PhraseCount>,
    pub tone_label: String,
    /// Exactly what goes into the cleanup prompt, if anything yet.
    pub prompt_summary: Option<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum StyleError {
    #[error("Couldn't save your style: {message}")]
    Io { message: String },
}

/// The user's passive style profile: whimpr-core's `StyleStore`, the same
/// counting the desktop does, saved to `path` after every change.
#[derive(uniffi::Object)]
pub struct StyleProfile {
    path: PathBuf,
    store: Mutex<StyleStore>,
}

#[uniffi::export]
impl StyleProfile {
    #[uniffi::constructor]
    pub fn open(path: String) -> Arc<Self> {
        let path = PathBuf::from(path);
        let store = StyleStore::load(&path);
        Arc::new(Self { path, store: Mutex::new(store) })
    }

    /// Learns from one dictation or piece of writing, into `mode`'s profile.
    pub fn observe(&self, text: String, mode: StyleMode) -> Result<(), StyleError> {
        self.observe_all(vec![text], mode).map(|_| ())
    }

    /// Learns from many samples (an import) with one save, into `mode`'s
    /// profile. Returns how many had any words.
    pub fn observe_all(&self, samples: Vec<String>, mode: StyleMode) -> Result<u32, StyleError> {
        let mode = mode.into();
        let mut store = self.lock();
        let before = store.summary(mode).samples;
        for sample in &samples {
            store.observe(mode, sample);
        }
        let learned = store.summary(mode).samples - before;
        if learned > 0 {
            self.save(&store)?;
        }
        Ok(learned)
    }

    pub fn overview(&self, mode: StyleMode) -> StyleOverview {
        let mode = mode.into();
        let store = self.lock();
        let summary = store.summary(mode);
        let counts = |items: Vec<(String, u32)>| {
            items.into_iter().map(|(phrase, count)| PhraseCount { phrase, count }).collect()
        };
        StyleOverview {
            samples: summary.samples,
            min_samples_to_apply: summary.min_samples_to_apply,
            ready: summary.ready,
            avg_sentence_words: summary.avg_sentence_words,
            contraction_rate: summary.contraction_rate,
            top_fillers: counts(summary.top_fillers),
            top_phrases: counts(summary.top_phrases),
            tone_label: summary.tone_label,
            prompt_summary: store.summary_for_prompt(mode),
        }
    }

    /// The one-line description for `mode`'s cleanup prompt, once there's enough.
    pub fn prompt_summary(&self, mode: StyleMode) -> Option<String> {
        self.lock().summary_for_prompt(mode.into())
    }

    pub fn reset(&self, mode: StyleMode) -> Result<(), StyleError> {
        let mut store = self.lock();
        store.reset(mode.into());
        self.save(&store)
    }
}

impl StyleProfile {
    fn lock(&self) -> MutexGuard<'_, StyleStore> {
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, store: &StyleStore) -> Result<(), StyleError> {
        store.save(&self.path).map_err(|e| StyleError::Io { message: e.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh file path per call. Tests run in parallel, and a clock reading
    /// alone can repeat, so a counter keeps every path unique.
    fn temp_path() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "whimpr-ffi-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        dir.join("store.json").to_string_lossy().into_owned()
    }

    const CASUAL: &[&str] = &[
        "so I don't really think we should ship it on Friday, you know",
        "I'm gonna check with Sam, I don't think it's ready yet",
        "honestly it's fine, we don't need another meeting about it",
        "I'll send it over tonight, it's basically done",
        "yeah I don't mind, it's up to you honestly",
        "I think it's good, I'll look at it again tomorrow",
    ];

    #[test]
    fn style_needs_five_samples_before_it_shapes_cleanup() {
        let style = StyleProfile::open(temp_path());
        assert_eq!(
            style.observe_all(CASUAL[..4].iter().map(|s| s.to_string()).collect(), StyleMode::General).unwrap(),
            4
        );
        assert!(!style.overview(StyleMode::General).ready);
        assert!(style.prompt_summary(StyleMode::General).is_none());

        style.observe(CASUAL[4].into(), StyleMode::General).unwrap();
        let overview = style.overview(StyleMode::General);
        assert!(overview.ready);
        assert!(overview.contraction_rate > 0.9, "all contracted: {}", overview.contraction_rate);
        let summary = style.prompt_summary(StyleMode::General).expect("a summary once ready");
        assert!(summary.contains("contractions"), "{summary}");
        assert_eq!(overview.prompt_summary.as_deref(), Some(summary.as_str()));
    }

    #[test]
    fn style_persists_and_resets() {
        let path = temp_path();
        StyleProfile::open(path.clone())
            .observe_all(CASUAL.iter().map(|s| s.to_string()).collect(), StyleMode::General)
            .unwrap();
        let reopened = StyleProfile::open(path.clone());
        assert_eq!(reopened.overview(StyleMode::General).samples, CASUAL.len() as u32);

        reopened.reset(StyleMode::General).unwrap();
        assert_eq!(StyleProfile::open(path).overview(StyleMode::General).samples, 0);
    }

    #[test]
    fn style_keeps_modes_independent() {
        let style = StyleProfile::open(temp_path());
        style.observe_all(CASUAL.iter().map(|s| s.to_string()).collect(), StyleMode::Email).unwrap();
        assert!(style.overview(StyleMode::Email).ready);
        assert_eq!(style.overview(StyleMode::EssayReport).samples, 0);
    }

    #[test]
    fn style_skips_empty_samples() {
        let style = StyleProfile::open(temp_path());
        assert_eq!(
            style
                .observe_all(vec!["".into(), "   ".into(), "hello there".into()], StyleMode::General)
                .unwrap(),
            1
        );
    }

    #[test]
    fn style_summary_reaches_the_prompt() {
        let style = StyleProfile::open(temp_path());
        style.observe_all(CASUAL.iter().map(|s| s.to_string()).collect(), StyleMode::General).unwrap();
        let summary = style.prompt_summary(StyleMode::General).unwrap();
        let request = prepare_cleanup("um so send it tonight".into(), Level::Light, vec![], None, Some(summary.clone()));
        let everything = format!("{}\n{}", request.system, request.messages.last().unwrap().content);
        assert!(everything.contains(&summary), "the style summary is in the request");
    }

    #[test]
    fn dictionary_starts_empty_and_persists() {
        let path = temp_path();
        let dict = UserDictionary::open(path.clone());
        assert!(dict.entries().is_empty());

        dict.add("Manvi".into(), vec!["monvi".into(), " manvee ".into(), "".into()]).unwrap();
        let reopened = UserDictionary::open(path);
        assert_eq!(
            reopened.entries(),
            vec![DictEntry { correct: "Manvi".into(), mishears: vec!["monvi".into(), "manvee".into()], learned: false }]
        );
    }

    #[test]
    fn dictionary_file_is_the_desktop_format() {
        let path = temp_path();
        UserDictionary::open(path.clone()).add("ChargeBee".into(), vec!["charge bee".into()]).unwrap();
        let desktop = DictionaryStore::load(std::path::Path::new(&path));
        assert_eq!(desktop.entries[0].correct, "ChargeBee");
        assert_eq!(desktop.entries[0].source, DictSource::Manual);
    }

    #[test]
    fn dictionary_merges_and_removes_ignoring_case() {
        let dict = UserDictionary::open(temp_path());
        dict.add("Manvi".into(), vec!["monvi".into()]).unwrap();
        dict.add("manvi".into(), vec!["manvie".into(), "Monvi".into()]).unwrap();
        assert_eq!(dict.entries().len(), 1);
        assert_eq!(dict.entries()[0].mishears, vec!["monvi", "manvie"]);

        assert!(dict.remove("MANVI".into()).unwrap());
        assert!(!dict.remove("Manvi".into()).unwrap());
        assert!(dict.entries().is_empty());
    }

    #[test]
    fn dictionary_rejects_empty_words_and_drops_self_mishears() {
        let dict = UserDictionary::open(temp_path());
        assert!(matches!(dict.add("   ".into(), vec![]), Err(DictionaryError::EmptyWord)));
        dict.add(" Koby ".into(), vec!["koby".into(), "Kobe".into()]).unwrap();
        assert_eq!(dict.entries()[0].correct, "Koby");
        assert_eq!(dict.entries()[0].mishears, vec!["Kobe"]);
    }

    #[test]
    fn dictionary_picks_relevant_words_for_the_prompt() {
        let dict = UserDictionary::open(temp_path());
        dict.add("Manvi".into(), vec!["monvi".into()]).unwrap();
        dict.add("ChargeBee".into(), vec![]).unwrap();

        let relevant = dict.relevant("send the deck to monvi please".into());
        assert_eq!(relevant, vec![Vocab { correct: "Manvi".into(), mishears: vec!["monvi".into()] }]);
        assert!(dict.relevant("renew charge bee".into()).iter().any(|v| v.correct == "ChargeBee"));
        assert!(dict.relevant("the weather is nice".into()).is_empty());
        assert_eq!(dict.words(), vec!["Manvi", "ChargeBee"]);
    }

    #[test]
    fn dictionary_save_failure_is_reported() {
        // A path under a regular file can't be created.
        let file = temp_path();
        std::fs::create_dir_all(std::path::Path::new(&file).parent().unwrap()).unwrap();
        std::fs::write(&file, "{}").unwrap();
        let dict = UserDictionary::open(format!("{file}/dictionary.json"));
        assert!(matches!(dict.add("Word".into(), vec![]), Err(DictionaryError::Io { .. })));
    }

    #[test]
    fn matches_the_desktop_prompt() {
        let raw = "um so can you send the report to monvi tomorrow";
        let vocab = vec![Vocab { correct: "Manvi".into(), mishears: vec!["monvi".into()] }];
        let request = prepare_cleanup(raw.into(), Level::Light, vocab.clone(), None, None);

        let ctx = CleanupContext {
            level: CleanupLevel::Light,
            vocab: vec![VocabEntry { correct: "Manvi".into(), mishears: vec!["monvi".into()] }],
            ..Default::default()
        };
        let desktop = cleanup::build_messages(&cleanup::pre_normalize_layout(raw), &ctx);
        assert!(!request.skip_model);
        assert_eq!(request.system, desktop[0].content);
        assert_eq!(request.messages.len(), desktop.len() - 1);
        for (ours, theirs) in request.messages.iter().zip(&desktop[1..]) {
            assert_eq!(ours.role, theirs.role);
            assert_eq!(ours.content, theirs.content);
        }
        assert!(request.messages.last().unwrap().content.contains("Manvi"));
    }

    #[test]
    fn level_none_skips_the_model_and_keeps_raw() {
        let request = prepare_cleanup("hello new line world".into(), Level::Off, vec![], None, None);
        assert!(request.skip_model);
        assert_eq!(request.fallback, "hello new line world");
    }

    #[test]
    fn empty_transcript_skips_the_model() {
        assert!(prepare_cleanup("  ".into(), Level::Light, vec![], None, None).skip_model);
    }

    #[test]
    fn spoken_line_breaks_survive_in_the_fallback() {
        let request = prepare_cleanup("first new line second".into(), Level::Light, vec![], None, None);
        assert!(request.fallback.contains('\n'), "{:?}", request.fallback);
        assert!(!request.fallback.contains("[["));
        assert!(request.messages.last().unwrap().content.contains("[[NL]]"));
    }

    #[test]
    fn accepts_a_light_cleanup() {
        let fallback = "um so I think we should uh ship it on Friday".to_string();
        let outcome = finish_cleanup("I think we should ship it on Friday.".into(), fallback, Level::Light);
        assert!(outcome.accepted, "{:?}", outcome.rejection);
        assert_eq!(outcome.text, "I think we should ship it on Friday.");
    }

    #[test]
    fn rejects_an_assistant_reply_and_falls_back() {
        let fallback = "what time is the meeting".to_string();
        let outcome = finish_cleanup(
            "Sure, the meeting is at 3pm. Let me know if you need anything else!".into(),
            fallback.clone(),
            Level::Light,
        );
        assert!(!outcome.accepted);
        assert_eq!(outcome.text, fallback);
    }

    #[test]
    fn rejects_a_lost_number() {
        let fallback = "the invoice total is 4250 dollars".to_string();
        let outcome = finish_cleanup("The invoice total is due.".into(), fallback.clone(), Level::Light);
        assert!(!outcome.accepted);
        assert_eq!(outcome.text, fallback);
    }

    #[test]
    fn empty_model_output_falls_back() {
        let outcome = finish_cleanup("   ".into(), "hello".into(), Level::Light);
        assert!(!outcome.accepted);
        assert_eq!(outcome.text, "hello");
    }
}
