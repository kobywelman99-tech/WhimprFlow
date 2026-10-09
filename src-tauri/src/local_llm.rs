//! Locates the local-LLM cleanup worker (a separate process, so llama.cpp and
//! whisper.cpp never link into the same binary) and its model, and builds the
//! supervised `whimpr_cleanup::LocalLlm` client.

use std::path::PathBuf;

/// Application-support dir: `~/Library/Application Support/WhimprFlow`.
fn app_support_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/WhimprFlow")
}

/// Find the worker binary: next to the app executable (bundled), else the dev build dir.
pub fn worker_bin_path() -> Option<PathBuf> {
    let exe_name = "whimpr-llm-worker";
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(exe_name);
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    // Dev fallback.
    let home = std::env::var("HOME").unwrap_or_default();
    let dev = PathBuf::from(home).join("WhimprFlow/target/release/whimpr-llm-worker");
    dev.exists().then_some(dev)
}

/// The local cleanup model path (same models dir as whisper/ASR). Prefer the
/// larger, much more capable Qwen3-4B if present (far better at
/// self-corrections and structure than the 1.5B); fall back to the 1.5B otherwise.
pub fn model_path() -> PathBuf {
    let dir = app_support_dir().join("models");
    for name in [
        "qwen3-4b-instruct-2507-q4_k_m.gguf",
        "qwen2.5-1.5b-instruct-q4_k_m.gguf",
    ] {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    dir.join("qwen2.5-1.5b-instruct-q4_k_m.gguf")
}

/// The supervised on-device model, if both the worker binary and a model file exist.
/// Nothing runs until `start()` is called on it.
pub fn build(timeout_ms: u32) -> Option<whimpr_cleanup::LocalLlm> {
    let bin = worker_bin_path()?;
    let model = model_path();
    if !model.exists() {
        eprintln!("[whimpr] local model not found at {}", model.display());
        return None;
    }
    let mut cfg = whimpr_cleanup::LocalConfig::new(bin, model);
    cfg.timeout = std::time::Duration::from_millis(u64::from(timeout_ms));
    Some(whimpr_cleanup::LocalLlm::new(cfg))
}
