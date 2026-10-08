// Typed wrappers over the Tauri command surface. In a plain browser (vite dev
// without the shell) the invoke import fails and we fall back to defaults so the
// Hub still renders for iteration.

export type CleanupMode = "raw" | "local" | "open_ai" | "anthropic";
export type CleanupLevel = "none" | "light" | "medium" | "high";
// How the target format for a dictation is chosen. "auto" infers it from the
// focused app (see cleanup/prompts.rs::format_mode_for_app); the other three
// are manual overrides that always win over that guess.
export type FormatMode = "auto" | "email" | "general" | "essay_report";

export interface Settings {
  cleanup_mode: CleanupMode;
  cleanup_level: CleanupLevel;
  openai_model: string;
  // API root for "OpenAI" mode — leave blank for OpenAI itself, or point at
  // an OpenAI-compatible endpoint like OpenRouter (https://openrouter.ai/api/v1).
  openai_base_url: string;
  anthropic_model: string;
  sound_on_start: boolean;
  // Passively learn the user's writing/speaking voice from raw transcripts
  // and fold a short summary into the cleanup request — see StylePane. Adds
  // no extra API calls, only a bit more text to the cleanup call that
  // already happens.
  style_learning_enabled: boolean;
  format_mode: FormatMode;
  // Re-read each dictation in context and fix words that don't make sense.
  review_enabled: boolean;
  // Longest an on-device cleanup request may take before the raw transcript is pasted.
  local_llm_timeout_ms: number;
  // Which speech recogniser is used: whisper (default) or Parakeet (opt-in, needs its model).
  asr_engine: "whisper_cpp" | "onnx_parakeet" | "fluid_audio_ane";
}

// Mirrors `permissions::Grant` in src-tauri. A bare boolean couldn't tell
// "nobody has asked yet" from "asked and turned down" — two states with
// completely different instructions for the reader.
export type Grant = "granted" | "not_asked" | "refused";

export interface Status {
  accessibility: boolean;
  microphone: boolean;
  input_monitoring: boolean;
  microphone_grant: Grant;
  // The app macOS is actually judging our microphone request as, when that
  // isn't us (a terminal that launched us, say). Null in the normal case.
  charged_to: string | null;
  // One sentence saying why the microphone row can't go green, when there's
  // something the reader couldn't otherwise have known. Null when there isn't.
  microphone_hint: string | null;
  has_openai_key: boolean;
  has_anthropic_key: boolean;
  // A speech model is installed, so dictation can run.
  has_asr_model: boolean;
}

// What the Hub falls back to before the first read lands (and in a plain
// browser preview, where there's no shell to ask).
export const UNKNOWN_STATUS: Status = {
  accessibility: false,
  microphone: false,
  input_monitoring: false,
  microphone_grant: "not_asked",
  charged_to: null,
  microphone_hint: null,
  has_openai_key: false,
  has_anthropic_key: false,
  has_asr_model: false,
};

// p50/p95 of one pipeline stage over the last 200 measured dictations. `n` is how many
// dictations the stage actually ran in (cleanup doesn't run in Raw mode).
export interface StageLatency {
  stage: "stop" | "resample" | "vad" | "asr" | "cleanup" | "review" | "paste" | "total";
  p50_ms: number;
  p95_ms: number;
  n: number;
}

export interface StatsSummary {
  total_words: number;
  total_sessions: number;
  total_speaking_secs: number;
  avg_wpm: number;
  best_wpm: number;
  words_today: number;
  wpm_today: number;
  day_streak: number;
  time_saved_secs: number;
  last7_words: number[];
  latency: StageLatency[];
}

export const EMPTY_STATS: StatsSummary = {
  total_words: 0,
  total_sessions: 0,
  total_speaking_secs: 0,
  avg_wpm: 0,
  best_wpm: 0,
  words_today: 0,
  wpm_today: 0,
  day_streak: 0,
  time_saved_secs: 0,
  last7_words: [0, 0, 0, 0, 0, 0, 0],
  latency: [],
};

export const DEFAULT_SETTINGS: Settings = {
  cleanup_mode: "open_ai",
  cleanup_level: "light",
  openai_model: "gpt-4o-mini",
  openai_base_url: "",
  anthropic_model: "claude-haiku-4-5",
  sound_on_start: true,
  style_learning_enabled: true,
  format_mode: "auto",
  review_enabled: true,
  local_llm_timeout_ms: 3000,
  asr_engine: "whisper_cpp",
};

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

// True when actually running inside the Tauri shell. Lets the mutation
// wrappers below tell "no shell present" (a plain browser dev preview, where
// swallowing the error is correct so the Hub still renders) apart from "the
// shell is here and the command genuinely failed" (where swallowing it would
// make the user think a save succeeded when it silently didn't).
function isTauri(): boolean {
  try {
    return "__TAURI_INTERNALS__" in window;
  } catch {
    return false;
  }
}

export async function getSettings(): Promise<Settings> {
  try {
    return await invoke<Settings>("get_settings");
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export async function setSettings(settings: Settings): Promise<void> {
  try {
    await invoke<void>("set_settings", { settings });
  } catch (e) {
    if (isTauri()) throw e; // real failure — let the caller surface it
  }
}

export async function getStatus(): Promise<Status> {
  try {
    return await invoke<Status>("get_status");
  } catch {
    return UNKNOWN_STATUS;
  }
}

// The permission heartbeat, pushed from Rust (`permissions::watch`) the moment
// macOS changes its mind. This is what makes the setup screen's promise —
// "turns green the moment macOS applies it, no relaunch needed" — true even
// when the Hub's own timer isn't running, which is exactly when the reader is
// off in System Settings doing the granting. Payload is the permission half of
// `Status`; the key fields ride along unchanged.
export type Permissions = Pick<
  Status,
  | "accessibility"
  | "microphone"
  | "input_monitoring"
  | "microphone_grant"
  | "charged_to"
  | "microphone_hint"
>;

export async function onPermissions(cb: (p: Permissions) => void): Promise<() => void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<Permissions>("whimpr://permissions", (e) => cb(e.payload));
  } catch {
    return () => {};
  }
}

// ── Speech models ────────────────────────────────────────────────────────────
// Mirrors `models::ModelDto` / `models::ProgressDto` in src-tauri.
export interface ModelInfo {
  id: string;
  title: string;
  blurb: string;
  size_bytes: number;
  // The file is in the models folder at the right size.
  installed: boolean;
  // This is the model speech recognition is using.
  active: boolean;
  // Installed, not in use, and can be switched to now (Parakeet <-> whisper).
  selectable: boolean;
  downloading: boolean;
}

export type ModelProgressState = "downloading" | "verifying" | "done" | "cancelled" | "error";

export interface ModelProgress {
  id: string;
  state: ModelProgressState;
  downloaded: number;
  total: number;
  // An error, or a note for the person such as "Restart WhimprFlow to start using it."
  message: string | null;
}

export async function listModels(): Promise<ModelInfo[]> {
  try {
    return await invoke<ModelInfo[]>("list_models");
  } catch {
    return [];
  }
}

export async function downloadModel(id: string): Promise<void> {
  await invoke<void>("download_model", { id });
}

export async function useModel(id: string): Promise<void> {
  await invoke<void>("use_model", { id });
}

export async function cancelModelDownload(): Promise<void> {
  try {
    await invoke<boolean>("cancel_model_download");
  } catch {
    /* nothing running */
  }
}

export async function onModelProgress(cb: (p: ModelProgress) => void): Promise<() => void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<ModelProgress>("whimpr://models/progress", (e) => cb(e.payload));
  } catch {
    return () => {};
  }
}

// The most recent loud diagnostic from the dictation pipeline (permission
// missing, hotkey tap dead, paste failed, empty transcript, …). Mirrors
// `diag::ErrorDto` in src-tauri/src/diag.rs.
export interface LastError {
  headline: string;
  detail: string;
}

export async function getLastError(): Promise<LastError | null> {
  try {
    return await invoke<LastError | null>("get_last_error");
  } catch {
    return null;
  }
}

export async function getStats(): Promise<StatsSummary> {
  try {
    const tz = new Date().getTimezoneOffset(); // minutes to add to local -> UTC
    return await invoke<StatsSummary>("get_stats", { tzOffsetMinutes: tz });
  } catch {
    return EMPTY_STATS;
  }
}

export async function requestMicrophone(): Promise<void> {
  try {
    await invoke<void>("request_microphone");
  } catch {
    /* browser preview */
  }
}

export async function requestAccessibility(): Promise<void> {
  try {
    await invoke<void>("request_accessibility");
  } catch {
    /* browser preview */
  }
}

export async function requestInputMonitoring(): Promise<void> {
  try {
    await invoke<void>("request_input_monitoring");
  } catch {
    /* browser preview */
  }
}

export async function setApiKey(provider: "openai" | "anthropic", key: string): Promise<void> {
  try {
    await invoke<void>("set_api_key", { provider, key });
  } catch (e) {
    if (isTauri()) throw e;
  }
}

// ── History ────────────────────────────────────────────────────────────────
export interface HistoryItem {
  ts_unix: number;
  text: string;
  app: string | null;
  words: number;
}

export async function getHistory(): Promise<HistoryItem[]> {
  try {
    return await invoke<HistoryItem[]>("get_history");
  } catch {
    return [];
  }
}

// ── Dictionary ───────────────────────────────────────────────────────────────
export interface DictEntry {
  correct: string;
  mishears: string[];
  auto: boolean;
}

export async function getDictionary(): Promise<DictEntry[]> {
  try {
    return await invoke<DictEntry[]>("get_dictionary");
  } catch {
    return [];
  }
}

export async function addDictionaryEntry(correct: string, mishears: string[]): Promise<void> {
  try {
    await invoke<void>("add_dictionary_entry", { correct, mishears });
  } catch (e) {
    if (isTauri()) throw e;
  }
}

export async function removeDictionaryEntry(correct: string): Promise<void> {
  try {
    await invoke<void>("remove_dictionary_entry", { correct });
  } catch (e) {
    if (isTauri()) throw e;
  }
}

// ── Voice calibration ────────────────────────────────────────────────────────
// Mirrors the calibration commands in src-tauri/src/lib.rs.
export interface CalibrationPrompt {
  id: number;
  tag: string;
  text: string;
}

// A sound the user tends to say differently, e.g. meant "th (think)" → heard "f (fan)".
export interface BrainPattern {
  meant: string;
  heard: string;
  rate: number;
  count: number;
  opportunities: number;
}

export interface BrainOverview {
  observations: number;
  learned_words: number;
  patterns: BrainPattern[];
}

export const EMPTY_BRAIN: BrainOverview = { observations: 0, learned_words: 0, patterns: [] };

export async function getCalibrationPrompts(count: number, seed: number): Promise<CalibrationPrompt[]> {
  try {
    return await invoke<CalibrationPrompt[]>("calibration_prompts", { count, seed });
  } catch {
    return [];
  }
}

export async function calibrationRecordStart(): Promise<void> {
  await invoke<void>("calibration_record_start");
}

/** Stops recording; resolves to what the recogniser heard. */
export async function calibrationRecordStop(): Promise<string> {
  return await invoke<string>("calibration_record_stop");
}

/** Teach the brain from a session; resolves to what it now knows. */
export async function learnFromCalibration(
  attempts: { expected: string; heard: string }[],
): Promise<BrainOverview> {
  return await invoke<BrainOverview>("calibration_learn", { attempts });
}

export async function getBrain(): Promise<BrainOverview> {
  try {
    return await invoke<BrainOverview>("get_brain");
  } catch {
    return EMPTY_BRAIN;
  }
}

export async function resetBrain(): Promise<BrainOverview> {
  return await invoke<BrainOverview>("reset_brain");
}

// ── Style ──────────────────────────────────────────────────────────────────
// Mirrors `whimpr_core::style::StyleSummary` in src-tauri.
export interface StyleSummary {
  samples: number;
  min_samples_to_apply: number;
  ready: boolean;
  avg_sentence_words: number;
  contraction_rate: number;
  top_fillers: [string, number][];
  top_phrases: [string, number][];
  tone_label: string;
}

export const EMPTY_STYLE_SUMMARY: StyleSummary = {
  samples: 0,
  min_samples_to_apply: 5,
  ready: false,
  avg_sentence_words: 0,
  contraction_rate: 0,
  top_fillers: [],
  top_phrases: [],
  tone_label: "",
};

// The three learned voice profiles — one per style mode. "auto" has no
// profile of its own; it resolves to email or general depending on the
// focused app (see `whimpr_core::style::resolve_style_mode`).
export type StyleProfileMode = "email" | "general" | "essay_report";

export const EMPTY_STYLE_SUMMARIES: Record<StyleProfileMode, StyleSummary> = {
  email: EMPTY_STYLE_SUMMARY,
  general: EMPTY_STYLE_SUMMARY,
  essay_report: EMPTY_STYLE_SUMMARY,
};

export async function getStyleSummaries(): Promise<Record<StyleProfileMode, StyleSummary>> {
  try {
    return await invoke<Record<StyleProfileMode, StyleSummary>>("get_style_summaries");
  } catch {
    return EMPTY_STYLE_SUMMARIES;
  }
}

export async function resetStyleProfile(mode: StyleProfileMode): Promise<void> {
  try {
    await invoke<void>("reset_style_profile", { mode });
  } catch (e) {
    if (isTauri()) throw e;
  }
}

