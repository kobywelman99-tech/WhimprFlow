//! Dictation usage stats — words dictated, speaking time, words-per-minute,
//! streaks, and estimated time saved vs typing. One small record is appended per
//! completed dictation and persisted as JSON (same dependency-light pattern as
//! [`crate::settings`] and [`crate::dictionary`]).
//!
//! All "today"/"streak" bucketing is done against a timezone offset the UI passes
//! in (minutes to add to local time to reach UTC, i.e. JS `getTimezoneOffset()`),
//! so day boundaries line up with the user's own clock without pulling in a
//! timezone crate.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Average typing speed (words/min) we compare speaking against for "time saved".
/// 45 wpm matches Wispr Flow's own typed baseline (they cite 45 typed vs ~220 spoken).
const TYPING_WPM_BASELINE: f64 = 45.0;

const DAY_SECS: i64 = 86_400;

/// Where the time went for one dictation, in milliseconds, measured from the key-up.
/// All zero means "not measured" (records from before timings existed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timings {
    /// Key-up until the microphone is stopped and the audio is in hand.
    pub stop_ms: u32,
    pub resample_ms: u32,
    /// Voice-activity detection (trimming and skipping silence).
    #[serde(default)]
    pub vad_ms: u32,
    /// Speech recognition.
    pub asr_ms: u32,
    /// The cleanup call (0 when cleanup didn't run).
    pub cleanup_ms: u32,
    /// The sense check (0 when it didn't run).
    pub review_ms: u32,
    pub paste_ms: u32,
    /// Key-up until the text is pasted.
    pub total_ms: u32,
}

impl Timings {
    /// Stage name and value, in pipeline order.
    pub fn stages(&self) -> [(&'static str, u32); 8] {
        [
            ("stop", self.stop_ms),
            ("resample", self.resample_ms),
            ("vad", self.vad_ms),
            ("asr", self.asr_ms),
            ("cleanup", self.cleanup_ms),
            ("review", self.review_ms),
            ("paste", self.paste_ms),
            ("total", self.total_ms),
        ]
    }
}

/// p50 and p95 of one stage over recent dictations. `n` counts the dictations where
/// the stage ran (a stage that didn't run, such as cleanup in Raw mode, isn't a zero).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageLatency {
    pub stage: String,
    pub p50_ms: u32,
    pub p95_ms: u32,
    pub n: u32,
}

/// How many of the most recent measured dictations the latency summary looks at.
pub const LATENCY_WINDOW: usize = 200;

/// One completed dictation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Seconds since the Unix epoch (UTC) when the dictation was committed.
    pub ts_unix: u64,
    /// Word count of the final inserted text.
    pub words: u32,
    /// Speaking duration in milliseconds.
    pub duration_ms: u32,
    /// Character count of the final inserted text.
    pub chars: u32,
    /// The cleaned/inserted text (for the Home history list). Older records may
    /// predate this field.
    #[serde(default)]
    pub text: String,
    /// Bundle id of the app the text was inserted into, if known.
    #[serde(default)]
    pub app: Option<String>,
    /// Where the time went. Older records predate this and read as all zero.
    #[serde(default)]
    pub timings: Timings,
}

/// A history row for the Hub Home list (newest first). Trimmed view of a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub ts_unix: u64,
    pub text: String,
    pub app: Option<String>,
    pub words: u32,
}

/// The persisted stats log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsStore {
    #[serde(default)]
    pub sessions: Vec<SessionRecord>,
}

/// Aggregated stats for the Hub. Everything the UI needs to draw the dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSummary {
    pub total_words: u64,
    pub total_sessions: u64,
    pub total_speaking_secs: f64,
    /// Lifetime average speaking speed (words/min).
    pub avg_wpm: u32,
    /// Fastest single dictation (words/min), ignoring trivially short ones.
    pub best_wpm: u32,
    pub words_today: u64,
    pub wpm_today: u32,
    /// Consecutive days (up to today) with at least one dictation.
    pub day_streak: u32,
    /// Estimated seconds saved vs typing the same words at [`TYPING_WPM_BASELINE`].
    pub time_saved_secs: f64,
    /// Words per local day, oldest first; index 6 is today, 0 is six days ago.
    pub last7_words: [u64; 7],
    /// p50/p95 per pipeline stage over the last [`LATENCY_WINDOW`] measured dictations.
    #[serde(default)]
    pub latency: Vec<StageLatency>,
}

/// Nearest-rank percentile (`p` in 0–100) of `values`; `None` if there are none.
/// With 100 values, p95 is the 95th smallest.
pub fn percentile(values: &[u32], p: f64) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((p.clamp(0.0, 100.0) / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.max(1) - 1).copied()
}

/// Count whitespace-delimited words. Matches how the cleanup layer thinks of words.
pub fn count_words(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

/// The local calendar day index for a UTC timestamp, given the UI's tz offset
/// (minutes to add to local to get UTC, per JS `Date.getTimezoneOffset()`).
fn local_day(ts_unix: u64, tz_offset_minutes: i32) -> i64 {
    let local = ts_unix as i64 - (tz_offset_minutes as i64) * 60;
    local.div_euclid(DAY_SECS)
}

/// Words/min from words and a duration, rounded; 0 for empty/instant sessions.
fn wpm(words: u64, secs: f64) -> u32 {
    if secs <= 0.0 || words == 0 {
        return 0;
    }
    (words as f64 / (secs / 60.0)).round() as u32
}

impl StatsStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }

    /// Append one completed dictation, without timings.
    pub fn record(
        &mut self,
        words: u32,
        duration_ms: u32,
        chars: u32,
        ts_unix: u64,
        text: String,
        app: Option<String>,
    ) {
        self.push(SessionRecord { ts_unix, words, duration_ms, chars, text, app, timings: Timings::default() });
    }

    /// Append one completed dictation, including where its time went.
    pub fn push(&mut self, record: SessionRecord) {
        self.sessions.push(record);
    }

    /// p50/p95 per stage over the last [`LATENCY_WINDOW`] dictations that were measured.
    pub fn latency(&self) -> Vec<StageLatency> {
        let recent: Vec<Timings> = self
            .sessions
            .iter()
            .rev()
            .map(|s| s.timings)
            .filter(|t| t.total_ms > 0)
            .take(LATENCY_WINDOW)
            .collect();
        Timings::default()
            .stages()
            .iter()
            .enumerate()
            .map(|(i, (stage, _))| {
                let ran: Vec<u32> = recent.iter().map(|t| t.stages()[i].1).filter(|&ms| ms > 0).collect();
                StageLatency {
                    stage: stage.to_string(),
                    p50_ms: percentile(&ran, 50.0).unwrap_or(0),
                    p95_ms: percentile(&ran, 95.0).unwrap_or(0),
                    n: ran.len() as u32,
                }
            })
            .collect()
    }

    /// The most recent `limit` dictations, newest first, for the Home history list.
    pub fn history(&self, limit: usize) -> Vec<HistoryItem> {
        self.sessions
            .iter()
            .rev()
            .filter(|s| !s.text.is_empty())
            .take(limit)
            .map(|s| HistoryItem {
                ts_unix: s.ts_unix,
                text: s.text.clone(),
                app: s.app.clone(),
                words: s.words,
            })
            .collect()
    }

    /// Aggregate everything the dashboard shows. `now_unix` and `tz_offset_minutes`
    /// come from the caller so day math matches the user's local clock (and so the
    /// aggregation stays pure/testable).
    pub fn summary(&self, tz_offset_minutes: i32, now_unix: u64) -> StatsSummary {
        let total_words: u64 = self.sessions.iter().map(|s| s.words as u64).sum();
        let total_sessions = self.sessions.len() as u64;
        let total_speaking_secs: f64 =
            self.sessions.iter().map(|s| s.duration_ms as f64 / 1000.0).sum();

        let avg_wpm = wpm(total_words, total_speaking_secs);

        // Best WPM, ignoring blips that inflate the number (need real words + time).
        let best_wpm = self
            .sessions
            .iter()
            .filter(|s| s.words >= 3 && s.duration_ms >= 1000)
            .map(|s| wpm(s.words as u64, s.duration_ms as f64 / 1000.0))
            .max()
            .unwrap_or(0);

        let today = local_day(now_unix, tz_offset_minutes);

        let mut words_today: u64 = 0;
        let mut secs_today: f64 = 0.0;
        let mut last7_words = [0u64; 7];
        for s in &self.sessions {
            let day = local_day(s.ts_unix, tz_offset_minutes);
            if day == today {
                words_today += s.words as u64;
                secs_today += s.duration_ms as f64 / 1000.0;
            }
            let ago = today - day; // 0 = today, 6 = six days ago
            if (0..7).contains(&ago) {
                last7_words[(6 - ago) as usize] += s.words as u64;
            }
        }
        let wpm_today = wpm(words_today, secs_today);

        // Streak: consecutive days with activity, up to today. A day with no
        // dictations yet doesn't break the streak until it's fully past, so if
        // today is still empty we start counting from yesterday.
        use std::collections::HashSet;
        let active: HashSet<i64> = self
            .sessions
            .iter()
            .map(|s| local_day(s.ts_unix, tz_offset_minutes))
            .collect();
        let mut day_streak = 0u32;
        let mut d = if active.contains(&today) { today } else { today - 1 };
        while active.contains(&d) {
            day_streak += 1;
            d -= 1;
        }

        // Time saved: how long these words would take to type at the baseline,
        // minus the time actually spent speaking. Never negative.
        let typed_secs = total_words as f64 / TYPING_WPM_BASELINE * 60.0;
        let time_saved_secs = (typed_secs - total_speaking_secs).max(0.0);

        StatsSummary {
            total_words,
            total_sessions,
            total_speaking_secs,
            avg_wpm,
            best_wpm,
            words_today,
            wpm_today,
            day_streak,
            time_saved_secs,
            last7_words,
            latency: self.latency(),
        }
    }
}

#[cfg(test)]
mod tests {
    fn timed(total: u32, asr: u32, cleanup: u32) -> SessionRecord {
        SessionRecord {
            ts_unix: 1,
            words: 3,
            duration_ms: 1000,
            chars: 10,
            text: "a b c".into(),
            app: None,
            timings: Timings { stop_ms: 5, resample_ms: 1, vad_ms: 8, asr_ms: asr, cleanup_ms: cleanup, review_ms: 0, paste_ms: 210, total_ms: total },
        }
    }

    #[test]
    fn latency_is_p50_p95_per_stage_and_skips_stages_that_did_not_run() {
        let mut s = StatsStore::default();
        for i in 1..=100u32 {
            // Cleanup only ran for the first 50 dictations.
            s.push(timed(1000 + i, i, if i <= 50 { i * 10 } else { 0 }));
        }
        let lat = s.latency();
        let by = |name: &str| lat.iter().find(|l| l.stage == name).unwrap().clone();
        assert_eq!(lat.len(), 8);
        let asr = by("asr");
        assert_eq!((asr.p50_ms, asr.p95_ms, asr.n), (50, 95, 100));
        let cleanup = by("cleanup");
        assert_eq!((cleanup.p50_ms, cleanup.p95_ms, cleanup.n), (250, 480, 50));
        let review = by("review");
        assert_eq!((review.p50_ms, review.n), (0, 0), "a stage that never ran has no latency, not a fast one");
        assert_eq!(by("total").p95_ms, 1095);
    }

    #[test]
    fn unmeasured_records_are_ignored_and_old_files_still_load() {
        let old = r#"{"sessions":[{"ts_unix":1,"words":2,"duration_ms":900,"chars":9}]}"#;
        let s: StatsStore = serde_json::from_str(old).unwrap();
        assert_eq!(s.sessions[0].timings, Timings::default());
        assert!(s.latency().iter().all(|l| l.n == 0));
        assert_eq!(s.summary(0, 100).total_words, 2);
        // And the summary round-trips with the new field.
        let json = serde_json::to_string(&s.summary(0, 100)).unwrap();
        assert!(json.contains("\"latency\""));
    }

    #[test]
    fn only_the_most_recent_dictations_count() {
        let mut s = StatsStore::default();
        for _ in 0..LATENCY_WINDOW {
            s.push(timed(100, 100, 0));
        }
        // 200 older, slower ones should have been pushed out of the window.
        let mut slow = StatsStore::default();
        for _ in 0..50 {
            slow.push(timed(9000, 9000, 0));
        }
        for r in s.sessions.drain(..) {
            slow.push(r);
        }
        let asr = slow.latency().into_iter().find(|l| l.stage == "asr").unwrap();
        assert_eq!((asr.p95_ms, asr.n), (100, LATENCY_WINDOW as u32));
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let v: Vec<u32> = (1..=100).collect();
        assert_eq!(percentile(&v, 50.0), Some(50));
        assert_eq!(percentile(&v, 95.0), Some(95));
        assert_eq!(percentile(&v, 100.0), Some(100));
        assert_eq!(percentile(&v, 0.0), Some(1));
        // Unsorted input, small sets, and the empty case.
        assert_eq!(percentile(&[30, 10, 20], 50.0), Some(20));
        assert_eq!(percentile(&[7], 95.0), Some(7));
        assert_eq!(percentile(&[], 50.0), None);
    }

    use super::*;

    // A fixed "now": 2021-01-08 12:00:00 UTC.
    const NOW: u64 = 1_610_107_200;
    const DAY: u64 = 86_400;

    #[test]
    fn counts_words() {
        assert_eq!(count_words("  hello   there  world "), 3);
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn aggregates_totals_and_wpm() {
        let mut s = StatsStore::default();
        // 60 words in 60s -> 60 wpm.
        s.record(60, 60_000, 300, NOW, String::new(), None);
        // 30 words in 15s -> 120 wpm.
        s.record(30, 15_000, 150, NOW, String::new(), None);
        let sum = s.summary(0, NOW);
        assert_eq!(sum.total_words, 90);
        assert_eq!(sum.total_sessions, 2);
        // 90 words / 75s = 72 wpm.
        assert_eq!(sum.avg_wpm, 72);
        assert_eq!(sum.best_wpm, 120);
        assert_eq!(sum.words_today, 90);
    }

    #[test]
    fn streak_counts_consecutive_days_including_gap_today() {
        let mut s = StatsStore::default();
        // Activity yesterday, day-before, and three days ago (but NOT today).
        s.record(10, 5_000, 50, NOW - DAY, String::new(), None);
        s.record(10, 5_000, 50, NOW - 2 * DAY, String::new(), None);
        s.record(10, 5_000, 50, NOW - 3 * DAY, String::new(), None);
        // Gap at 4 days ago, then one more.
        s.record(10, 5_000, 50, NOW - 5 * DAY, String::new(), None);
        let sum = s.summary(0, NOW);
        // Today empty -> start at yesterday; 3 consecutive days back, then a gap.
        assert_eq!(sum.day_streak, 3);
        assert_eq!(sum.words_today, 0);
    }

    #[test]
    fn last7_buckets_by_local_day() {
        let mut s = StatsStore::default();
        s.record(5, 3_000, 25, NOW, String::new(), None); // today
        s.record(7, 3_000, 35, NOW - 2 * DAY, String::new(), None); // 2 days ago
        let sum = s.summary(0, NOW);
        assert_eq!(sum.last7_words[6], 5); // today
        assert_eq!(sum.last7_words[4], 7); // two days ago
        assert_eq!(sum.last7_words[5], 0);
    }
}
