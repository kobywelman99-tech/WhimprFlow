# WhimprFlow

A **local-first voice dictation app for macOS** — hold a key, speak, and clean text lands wherever your cursor is. Speech is transcribed on-device with Whisper and cleaned up (filler removal, self-corrections, punctuation, lists/newlines) by a local LLM, with an optional cloud path. It re-creates the workflow of a Wispr-Flow-style dictation tool from scratch, with its own name, palette, and code.

> ⚠️ **This is a proof of concept, vibe-coded in a few hours.** It works and the core loop is real, but it is rough and needs a lot of polish, testing, and hardening before it's anything like production quality. Treat it as a starting point, not a finished product.

---

## Platform status

**macOS 14+ only** (developed and tested on Apple Silicon), build-from-source for now. Windows support was removed from the desktop app; the shared crates stay platform-agnostic, so it can be brought back later. An iOS app lives in `ios/`.

---

## What's in it

- **On-device ASR** — Whisper (via `whisper.cpp`), running on the GPU. Ships a small English model by default; larger models are auto-preferred if present.
- **Local LLM cleanup** — Qwen3-4B-Instruct (via `llama.cpp`) runs as a separate worker process and cleans the transcript: removes fillers, resolves spoken self-corrections ("meet at 2… no wait, 3" → "3"), applies spoken punctuation, and formats lists/paragraphs. Deterministic gates guard against over-editing, with a raw-transcript fallback.
- **Optional cloud cleanup** — OpenAI (default) / Anthropic, behind one trait. Keys are stored locally in a user-only file (`0600`), **never in a file**.
- **Floating pill UI** — a small always-on-top bar showing idle / recording / processing states.
- **Personal dictionary + auto-learn** — teach it names and terms; on macOS a post-paste Accessibility observer watches for a one-word correction and learns it automatically (conservative filters to avoid junk).
- **Usage stats** — words dictated, words-per-minute, day streak, time saved, 7-day activity, all stored locally.

## Architecture

Tauri v2 (Rust core + React/TypeScript webviews). Platform-agnostic logic lives in `crates/whimpr-core` (state machine, cleanup prompts/gates, dictionary, stats). ASR, audio, and the LLM worker are separate crates. The Tauri app in `src-tauri/` hosts the UI and wires the native hotkey, paste and Accessibility code (`hotkey.rs`, `paste.rs`, `autolearn.rs`).

```
crates/
  whimpr-core/       state machine, cleanup (prompts/gates/levels), dictionary, stats
  whimpr-asr/        Whisper ASR
  whimpr-audio/      mic capture + resampling
  whimpr-cleanup/    OpenAI / Anthropic cloud providers
  whimpr-llm-worker/ local llama.cpp cleanup worker (separate process)
src-tauri/           Tauri shell: hotkey, paste, autolearn
ui/                  React Hub + overlay pill
docs/                spec, architecture notes, research
```

## Build (macOS)

Requires Rust (stable), Node + pnpm, and the Xcode command-line tools.

```bash
cd ui && pnpm install && cd ..
# If pnpm reports [ERR_PNPM_IGNORED_BUILDS] (esbuild's postinstall was
# skipped), this is a one-time, harmless-if-not-needed insurance step:
pnpm --dir ui approve-builds --all
# Dev:
./dev.sh
# Or a signed .app bundle — build ONLY via `tauri build`; a bare `cargo
# build` + manual codesign will NOT bundle the UI and can drop TCC grants:
ui/node_modules/.bin/tauri build --bundles app
```

Models are **not** committed (they're multi-GB) — see **[docs/MODELS.md](docs/MODELS.md)**
for the exact file + download link (short version: `ggml-base.en.bin` is
required, a Qwen GGUF for local/offline cleanup is optional).

## Troubleshooting: "I hold the key, speak, and nothing gets typed"

WhimprFlow now surfaces the exact reason on the pill and in the Hub instead of
failing silently (previously this only ever logged to a terminal, which is
why it looked like nothing was happening at all). If you still hit this:

- **macOS — Accessibility.** This is the #1 cause. Open **System Settings →
  Privacy & Security → Accessibility** and confirm WhimprFlow is toggled ON.
  The Hub's onboarding screen blocks you here on first launch; if you granted
  it once and it still doesn't work, especially after **rebuilding** the app,
  see the next point.
- **macOS — "granted but still nothing" after a rebuild.** Every local
  `tauri build` produces a differently-signed binary, and macOS can leave a
  stale Accessibility entry for the old signature that *looks* granted but
  isn't. Fix: in System Settings → Privacy & Security → Accessibility, remove
  WhimprFlow with the **−** button and re-add it (or toggle it off/on), then
  relaunch. WhimprFlow's pill and Hub will now show "Fn key isn't wired up"
  when this happens instead of just doing nothing.
- **No speech model.** See [docs/MODELS.md](docs/MODELS.md) — dictation needs
  a Whisper `ggml-*.bin` file placed by hand; the app doesn't download one for
  you.
- Still stuck? Run the app from a terminal (`./dev.sh`) and hold the key once — every failure
  path also logs a `[whimpr]`-prefixed line explaining what happened.

## Notes & disclaimers

- **Not affiliated with, endorsed by, or connected to Wispr Flow or any other product.** WhimprFlow is an independent, from-scratch reimplementation of the dictation workflow, with its own name, branding, colors, strings, and code. No third-party code or assets are included.
- **Proof of concept.** Rushed, under-tested, and missing plenty (auto-learn is conservative, no installer/notarization/signing pipeline, error handling is thin). Contributions and fixes welcome.
- **Privacy.** ASR and default cleanup run on-device. Cloud cleanup is opt-in and only sends the transcript (not audio) to the provider you choose. API keys never touch disk in plaintext.

## License

MIT — see [LICENSE](LICENSE).
