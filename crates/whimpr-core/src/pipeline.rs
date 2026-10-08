//! Everything that happens to a transcript after speech recognition and before paste:
//! known-mishear fixes, cleanup with its deterministic gates and fallbacks, and the
//! sense check. Pure over explicit inputs (no statics, no I/O besides the providers
//! passed in), so the app and the eval harness run exactly the same code.
//!
//! The shell owns everything with side effects: logging, user-facing notices, style
//! learning, timing and paste. It reads [`Output`] to know what to tell the user.

use crate::brain::Brain;
use crate::cleanup::{self, CleanupContext, CleanupProvider, GateReason, GateVerdict};
use crate::dictionary::DictionaryStore;
use crate::review::{self, Change};
use crate::settings::{CleanupMode, Settings};

/// How many dictionary entries are shown to the model per dictation.
pub const MAX_PROMPT_VOCAB: usize = 15;

/// The providers available right now. A missing one means "no key" or "not ready".
#[derive(Default, Clone, Copy)]
pub struct Providers<'a> {
    pub openai: Option<&'a dyn CleanupProvider>,
    pub anthropic: Option<&'a dyn CleanupProvider>,
    /// The on-device model, if it is loaded and ready.
    pub local: Option<&'a dyn CleanupProvider>,
}

/// Everything the pipeline reads. All borrowed or cheap to build per dictation.
pub struct Env<'a> {
    pub settings: &'a Settings,
    pub dictionary: &'a DictionaryStore,
    /// The learned writing-style summary for this dictation's mode, if there is one.
    pub style_summary: Option<String>,
    /// Bundle id of the app being pasted into, for per-app formatting.
    pub app_bundle_id: Option<String>,
    pub providers: Providers<'a>,
}

/// What the cleanup step did.
#[derive(Debug, Clone, PartialEq)]
pub enum CleanupStep {
    /// Raw mode or level None: nothing was sent anywhere.
    Skipped,
    Cleaned,
    /// The model's text was thrown away by a deterministic gate.
    GateRejected(GateReason),
    /// The provider errored; the gated raw transcript was used.
    Failed(String),
    /// No provider could run. `local` says whether Local mode was selected.
    Unavailable { local: bool },
}

/// What the sense check did.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewStep {
    /// Disabled, Raw/None level, or no cloud provider selected.
    NotRun,
    /// The message was fine, or too short to bother checking.
    Unchanged,
    Fixed(Vec<Change>),
    /// A fix was proposed and refused by the guards, or the call failed.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// What to paste.
    pub text: String,
    pub cleanup: CleanupStep,
    pub review: ReviewStep,
}

/// Rewrite known mishears: the dictionary's "also heard as" entries, then word pairs
/// the brain has seen repeat. Runs at every cleanup level, including Raw.
pub fn fix_mishears(raw: &str, dictionary: Option<&DictionaryStore>, brain: Option<&Brain>) -> String {
    let fixed = dictionary.map_or_else(|| raw.to_string(), |d| d.apply_replacements(raw));
    brain.map_or(fixed.clone(), |b| b.correct(&fixed))
}

/// Clean a (mishear-fixed) transcript per the settings, then give the result the
/// last read. Falls back to the raw transcript whenever cleanup is off, a provider is
/// unavailable or errors, or the gates reject the edit.
pub fn run(raw: &str, env: &Env) -> Output {
    let settings = env.settings;
    let level = settings.cleanup_level;
    // Nothing was said: don't spend a model call on it.
    if raw.trim().is_empty() || matches!(settings.cleanup_mode, CleanupMode::Raw) || level.bypasses_llm() {
        return Output { text: raw.to_string(), cleanup: CleanupStep::Skipped, review: ReviewStep::NotRun };
    }

    // Turn explicit spoken layout cues ("new line", "new paragraph") into break markers
    // up front: the model passes an opaque marker through reliably but mangles the
    // literal cue words. The model sees `raw_norm`; the gate and any raw fallback use
    // `raw_out` (markers restored to real breaks) so a "[[NL]]" token never reaches the
    // cursor and an explicit break is never lost.
    let raw_norm = cleanup::pre_normalize_layout(raw);
    let raw_out = cleanup::post_process(&raw_norm);
    let ctx = CleanupContext {
        level,
        vocab: env.dictionary.prefilter(&raw_norm, MAX_PROMPT_VOCAB),
        app_bundle_id: env.app_bundle_id.clone(),
        style_summary: env.style_summary.clone(),
        format_mode: settings.format_mode,
        ..Default::default()
    };

    // The selected provider, falling back to the local model when a cloud key is
    // missing (so cleanup still runs). Local mode uses the local model directly.
    let local = || env.providers.local.map(|p| p.cleanup(&raw_norm, &ctx));
    let result = match settings.cleanup_mode {
        CleanupMode::OpenAi => env.providers.openai.map(|p| p.cleanup(&raw_norm, &ctx)).or_else(local),
        CleanupMode::Anthropic => env.providers.anthropic.map(|p| p.cleanup(&raw_norm, &ctx)).or_else(local),
        CleanupMode::Local => local(),
        CleanupMode::Raw => None,
    };

    let (draft, cleanup) = match result {
        Some(Ok(cleaned)) => {
            // Deterministic safety net: leftover layout cues become real line breaks,
            // stray code fences go, blank lines are capped.
            let cleaned = cleanup::post_process(&cleaned);
            match cleanup::evaluate_gates(&raw_out, &cleaned, level) {
                GateVerdict::Pass => (cleaned, CleanupStep::Cleaned),
                GateVerdict::Fail(reason) => (raw_out, CleanupStep::GateRejected(reason)),
            }
        }
        Some(Err(e)) => (raw_out, CleanupStep::Failed(e.to_string())),
        None => (raw_out, CleanupStep::Unavailable { local: matches!(settings.cleanup_mode, CleanupMode::Local) }),
    };

    let (text, review) = sense_check(raw, draft, env);
    Output { text, cleanup, review }
}

/// The last read: does the message make sense? Cloud models only (the on-device model
/// is too small to judge); any failure keeps the message as it is.
fn sense_check(heard: &str, draft: String, env: &Env) -> (String, ReviewStep) {
    if !env.settings.review_enabled {
        return (draft, ReviewStep::NotRun);
    }
    let provider = match env.settings.cleanup_mode {
        CleanupMode::OpenAi => env.providers.openai,
        CleanupMode::Anthropic => env.providers.anthropic,
        _ => None,
    };
    let Some(provider) = provider else { return (draft, ReviewStep::NotRun) };
    let vocab = env.dictionary.prefilter(heard, MAX_PROMPT_VOCAB);
    let outcome = review::run(provider, heard, &draft, None, &vocab);
    if outcome.changed {
        (outcome.text, ReviewStep::Fixed(outcome.changes))
    } else if let Some(why) = outcome.rejected {
        (draft, ReviewStep::Refused(why))
    } else {
        (draft, ReviewStep::Unchanged)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::cleanup::{CleanupLevel, CleanupMsg, ProviderId};
    use crate::dictionary::DictSource;

    /// A provider that plays back scripted replies and records every request.
    struct Fake {
        replies: Mutex<VecDeque<Result<String, String>>>,
        calls: Mutex<Vec<Vec<CleanupMsg>>>,
    }

    impl Fake {
        fn new(replies: &[Result<&str, &str>]) -> Self {
            Self {
                replies: Mutex::new(replies.iter().map(|r| r.map(String::from).map_err(String::from)).collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
        fn last_user_message(&self) -> String {
            self.calls.lock().unwrap().last().unwrap().last().unwrap().content.clone()
        }
    }

    impl CleanupProvider for Fake {
        fn id(&self) -> ProviderId {
            ProviderId::Anthropic
        }
        fn complete(&self, messages: &[CleanupMsg]) -> anyhow::Result<String> {
            self.calls.lock().unwrap().push(messages.to_vec());
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra provider call")
                .map_err(|e| anyhow::anyhow!(e))
        }
    }

    fn settings(mode: CleanupMode, level: CleanupLevel, review: bool) -> Settings {
        Settings { cleanup_mode: mode, cleanup_level: level, review_enabled: review, ..Settings::default() }
    }

    fn env<'a>(settings: &'a Settings, dict: &'a DictionaryStore, providers: Providers<'a>) -> Env<'a> {
        Env { settings, dictionary: dict, style_summary: None, app_bundle_id: None, providers }
    }

    fn cloud<'a>(p: &'a Fake) -> Providers<'a> {
        Providers { openai: Some(p), anthropic: Some(p), local: None }
    }

    #[test]
    fn raw_mode_and_level_none_pass_the_text_through_untouched() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[]);
        for (mode, level) in [(CleanupMode::Raw, CleanupLevel::Light), (CleanupMode::OpenAi, CleanupLevel::None)] {
            let s = settings(mode, level, true);
            let out = run("um hello new line world", &env(&s, &dict, cloud(&p)));
            assert_eq!(out.text, "um hello new line world");
            assert_eq!(out.cleanup, CleanupStep::Skipped);
            assert_eq!(out.review, ReviewStep::NotRun);
        }
        assert_eq!(p.count(), 0, "nothing is sent anywhere");
    }

    #[test]
    fn an_empty_transcript_never_reaches_a_provider() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, true);
        for empty in ["", "   ", "\n"] {
            let out = run(empty, &env(&s, &dict, cloud(&p)));
            assert_eq!(out.cleanup, CleanupStep::Skipped);
            assert_eq!(out.review, ReviewStep::NotRun);
        }
        assert_eq!(p.count(), 0);
    }

    #[test]
    fn a_cleaned_result_is_used() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[Ok("I think we should ship it on Friday.")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, false);
        let out = run("um I think we should uh ship it on Friday", &env(&s, &dict, cloud(&p)));
        assert_eq!(out.text, "I think we should ship it on Friday.");
        assert_eq!(out.cleanup, CleanupStep::Cleaned);
    }

    #[test]
    fn spoken_layout_cues_become_markers_for_the_model_and_real_breaks_after() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[Err("down")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, false);
        let out = run("first point new line second point", &env(&s, &dict, cloud(&p)));
        assert!(!p.last_user_message().contains("new line"), "the model sees a marker, not the cue words");
        assert_eq!(out.text, "first point\nsecond point", "the fallback has the real break");
    }

    #[test]
    fn a_provider_error_pastes_the_gated_raw_transcript() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[Err("HTTP 503")]);
        let s = settings(CleanupMode::Anthropic, CleanupLevel::Light, false);
        let out = run("send it tomorrow", &env(&s, &dict, cloud(&p)));
        assert_eq!(out.text, "send it tomorrow");
        assert_eq!(out.cleanup, CleanupStep::Failed("HTTP 503".into()));
    }

    #[test]
    fn a_gate_rejection_pastes_raw() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[Ok("Sure, here is the rewritten text you asked for with plenty of extra words.")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, false);
        let out = run("send it tomorrow", &env(&s, &dict, cloud(&p)));
        assert_eq!(out.text, "send it tomorrow");
        assert!(matches!(out.cleanup, CleanupStep::GateRejected(_)), "{:?}", out.cleanup);
    }

    #[test]
    fn a_missing_cloud_key_falls_back_to_the_local_model() {
        let dict = DictionaryStore::default();
        let local = Fake::new(&[Ok("Send it tomorrow.")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, true);
        let providers = Providers { openai: None, anthropic: None, local: Some(&local) };
        let out = run("send it tomorrow", &env(&s, &dict, providers));
        assert_eq!(out.text, "Send it tomorrow.");
        assert_eq!(out.cleanup, CleanupStep::Cleaned);
        assert_eq!(out.review, ReviewStep::NotRun, "no sense check without the selected cloud provider");
        assert_eq!(local.count(), 1);
    }

    #[test]
    fn nothing_available_pastes_raw_and_says_which_kind_of_nothing() {
        let dict = DictionaryStore::default();
        let none = Providers::default();
        let cloud_mode = settings(CleanupMode::OpenAi, CleanupLevel::Light, true);
        let out = run("send it tomorrow", &env(&cloud_mode, &dict, none));
        assert_eq!((out.text.as_str(), &out.cleanup), ("send it tomorrow", &CleanupStep::Unavailable { local: false }));

        let local_mode = settings(CleanupMode::Local, CleanupLevel::Light, true);
        let out = run("send it tomorrow", &env(&local_mode, &dict, none));
        assert_eq!(out.cleanup, CleanupStep::Unavailable { local: true });
    }

    #[test]
    fn local_mode_uses_only_the_local_model() {
        let dict = DictionaryStore::default();
        let (local, cloud_p) = (Fake::new(&[Ok("Send it tomorrow.")]), Fake::new(&[]));
        let s = settings(CleanupMode::Local, CleanupLevel::Light, true);
        let providers = Providers { openai: Some(&cloud_p), anthropic: Some(&cloud_p), local: Some(&local) };
        let out = run("send it tomorrow", &env(&s, &dict, providers));
        assert_eq!(out.text, "Send it tomorrow.");
        assert_eq!((local.count(), cloud_p.count()), (1, 0));
        assert_eq!(out.review, ReviewStep::NotRun, "the on-device model doesn't judge sense");
    }

    #[test]
    fn dictionary_entries_reach_the_prompt_and_style_summary_when_present() {
        let mut dict = DictionaryStore::default();
        dict.add("Manvi", vec!["monvi".into()], DictSource::Manual);
        let p = Fake::new(&[Ok("Tell Manvi hi.")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, false);
        let mut e = env(&s, &dict, cloud(&p));
        e.style_summary = Some("short, casual sentences".into());
        run("tell monvi hi", &e);
        let prompt = p.last_user_message();
        assert!(prompt.contains("Manvi"), "{prompt}");
        assert!(prompt.contains("short, casual sentences"), "{prompt}");
    }

    #[test]
    fn the_sense_check_repairs_a_word_after_cleanup() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[
            Ok("Yes, I'll send you the rapport in an hour."),
            Ok("<fixed>Yes, I'll send you the report in an hour.</fixed>"),
        ]);
        let s = settings(CleanupMode::Anthropic, CleanupLevel::Light, true);
        let out = run("yes I'll send you the rapport in an hour", &env(&s, &dict, cloud(&p)));
        assert_eq!(out.text, "Yes, I'll send you the report in an hour.");
        assert_eq!(out.cleanup, CleanupStep::Cleaned);
        assert_eq!(out.review, ReviewStep::Fixed(vec![Change { from: "rapport".into(), to: "report".into() }]));
    }

    #[test]
    fn the_sense_check_can_be_turned_off_and_a_bad_fix_is_refused() {
        let dict = DictionaryStore::default();
        let off = settings(CleanupMode::OpenAi, CleanupLevel::Light, false);
        let p = Fake::new(&[Ok("Yes, I'll send you the rapport in an hour.")]);
        let out = run("yes I'll send you the rapport in an hour", &env(&off, &dict, cloud(&p)));
        assert_eq!((out.review, p.count()), (ReviewStep::NotRun, 1));

        let on = settings(CleanupMode::OpenAi, CleanupLevel::Light, true);
        let p = Fake::new(&[
            Ok("Yes, I'll send you the rapport in an hour."),
            Ok("<fixed>Yes, I'll send you the president in an hour.</fixed>"),
        ]);
        let out = run("yes I'll send you the rapport in an hour", &env(&on, &dict, cloud(&p)));
        assert_eq!(out.text, "Yes, I'll send you the rapport in an hour.");
        assert!(matches!(out.review, ReviewStep::Refused(_)), "{:?}", out.review);
    }

    #[test]
    fn a_failing_sense_check_keeps_the_cleaned_text() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[Ok("Yes, I'll send you the report in an hour."), Err("timeout")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, true);
        let out = run("yes I'll send you the report in an hour", &env(&s, &dict, cloud(&p)));
        assert_eq!(out.text, "Yes, I'll send you the report in an hour.");
        assert!(matches!(out.review, ReviewStep::Refused(ref m) if m.contains("timeout")), "{:?}", out.review);
    }

    #[test]
    fn short_messages_are_not_sent_for_a_sense_check() {
        let dict = DictionaryStore::default();
        let p = Fake::new(&[Ok("Okay thanks.")]);
        let s = settings(CleanupMode::OpenAi, CleanupLevel::Light, true);
        let out = run("okay thanks", &env(&s, &dict, cloud(&p)));
        assert_eq!((out.review, p.count()), (ReviewStep::Unchanged, 1));
    }

    #[test]
    fn known_mishears_are_fixed_from_the_dictionary_and_the_brain() {
        let mut dict = DictionaryStore::default();
        dict.add("Manvi", vec!["monvi".into()], DictSource::Manual);
        let mut brain = Brain::default();
        brain.observe("cold water", "cold wooder");
        brain.observe("water please", "wooder please");
        assert_eq!(fix_mishears("tell monvi", Some(&dict), None), "tell Manvi");
        assert_eq!(fix_mishears("a wooder", None, Some(&brain)), "a water");
        assert_eq!(fix_mishears("monvi wooder", Some(&dict), Some(&brain)), "Manvi water");
        assert_eq!(fix_mishears("nothing here", None, None), "nothing here");
    }
}
