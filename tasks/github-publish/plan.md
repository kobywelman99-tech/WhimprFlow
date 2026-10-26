# Implementation Plan: Publish Koby's WhimprFlow to his own GitHub

## Overview
Koby's improved WhimprFlow (local branch `ios-app`, 28 commits never pushed) goes to a new
**public** repo `github.com/kobywelman99-tech/WhimprFlow` as `main`, with a downloadable
dmg on GitHub Releases. A friend can then install it on a Mac (Apple Silicon or Intel)
without any files sent by hand. This is the single source of truth for that push.

**Where this plan lives:** `tasks/plan.md` and `tasks/todo.md` already hold the unfinished
iOS plan (29 unchecked items), so this plan is saved to **`tasks/github-publish/plan.md`**
and **`tasks/github-publish/todo.md`** and does not touch those files.

## Facts established (read-only checks, 2026-10-26)
- Original repo `Blueturboguy07/WhimprFlow` is MIT-licensed, so a fork is allowed as long as `LICENSE` stays.
- The logged-in `gh` account is `kobywelman99-tech`. It can't push to the original (403), and `kobywelman99-tech/WhimprFlow` doesn't exist yet.
- **A trial merge of `origin/main` is still open in the working tree, with conflict markers in 19 files.** It must be aborted first, and nothing gets built or committed before that.
- No API keys, private keys, `.env` or `.p12` files anywhere in `ios-app` history. The largest file is 3.6 MB.
- The only signing cert is `Apple Development: koby.welman99@gmail.com`, with no Developer ID and no notary profile. → **ad-hoc signed, not notarized** (Koby chose the free option).
- [scripts/build-macos.sh](scripts/build-macos.sh) refuses any identity that isn't Developer ID and defaults to Mann Bellani's cert. It needs an explicit ad-hoc mode.
- Only the `aarch64` Rust target is installed. The friend's Mac type is unknown → **universal build** (arm64 + x86_64).
- The branch downloads speech models inside the app (`crates/whimpr-models/src/registry.rs`), so no model goes in the dmg. The README still says "the app doesn't download one for you", which is outdated.
- `src-tauri/tauri.conf.json` on the branch says `0.1.1`, but the original already shipped `0.2.2`.

## Architecture Decisions
- **Push `ios-app` as the new repo's `main`, with full history and no merge of upstream `main`.** It's Koby's version, the history credits the original authors, and it avoids the 19-file conflict. The Linux, Windows and cloud-STT commits from upstream are left out on purpose.
- **Remotes:** rename the current `origin` to `upstream` (read-only reference) and add the new repo as `origin`. Push only `main`, with no upstream tags or branches.
- **Version `0.3.0`,** so it's clearly newer than the original's 0.2.2.
- **Ad-hoc signing as an opt-in `--adhoc` flag** in `build-macos.sh`. The Developer-ID guard stays the default, because that guard exists to prevent the v0.1.0 incident. `--adhoc` sets `APPLE_SIGNING_IDENTITY=-` and implies `--skip-notarize`.
- **Universal binary** via `--target universal-apple-darwin`. If a native dependency won't cross-compile, fall back to shipping two dmgs (arm64 + x86_64).
- **Commit identity:** new commits use Koby's GitHub noreply address, set in the repo-local config, so his personal email isn't published. Existing commits are not rewritten.
- The `ios/` folder stays in. The README marks the Mac app as the supported part.

## Task List

### Phase 1: Local foundation (no network writes)
- [ ] **Task 1: Return to a clean `ios-app`.** Run `git merge --abort`. Confirm `git status` is clean, `.git/MERGE_HEAD` is gone, and HEAD is `2e5bc29`. Set the repo-local `user.email` to `<id>+kobywelman99-tech@users.noreply.github.com` (id from `gh api user`).
  - *Verify:* `git status -sb` shows `## ios-app` and nothing else. `git grep -l '^<<<<<<<'` finds no conflict markers.
  - Size: XS · Deps: none
- [ ] **Task 2: Ad-hoc universal build works locally (highest risk, so it goes first).** Add `--adhoc` to `scripts/build-macos.sh`, following the existing flag-parsing and comment style. Run `rustup target add x86_64-apple-darwin`, then `pnpm install` in `ui/`. Build with `scripts/build-macos.sh --adhoc --target universal-apple-darwin`.
  - *Accept:* the dmg exists under `target/universal-apple-darwin/release/bundle/dmg/`. `lipo -archs` on the app binary (and the llm-worker binary, if bundled) prints `x86_64 arm64`. `codesign -dv` shows `Signature=adhoc`.
  - *Verify:* run `scripts/verify-macos.sh` if it applies. Launch the app, grant Mic and Accessibility, download the base model in the app, and confirm a dictation pastes text. If the x86_64 build fails, use the fallback: two single-arch dmgs.
  - Files: `scripts/build-macos.sh` · Size: S · Deps: 1

### Checkpoint A: the app builds and dictates from an ad-hoc universal dmg
- [ ] `cargo test --workspace` passes, and so does `pnpm --dir ui test` if present
- [ ] Koby confirms dictation works in the built app before anything goes public

### Phase 2: Make the repo his
- [ ] **Task 3: README, credit and version.** Bump the version to `0.3.0` in `src-tauri/tauri.conf.json` (plus `src-tauri/Cargo.toml` and `ui/package.json` if they carry it). README changes:
  - Add an **Install** section: download the dmg from Releases, drag it to Applications, open it once via System Settings → Privacy & Security → **Open Anyway**, then grant Mic and Accessibility.
  - Credit the original project with its link and note this is a fork.
  - Replace the outdated "app doesn't download a model" text.
  - Make the platform line say macOS 14+ on Apple Silicon or Intel.
  - Commit as `docs: install guide for the fork, credit upstream, bump to 0.3.0`.
  - *Verify:* `git grep -n "0.1.1" src-tauri ui/package.json` is empty. The README renders cleanly in VS Code preview. `LICENSE` is unchanged.
  - Files: `README.md`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `ui/package.json`, `scripts/build-macos.sh` (already done in Task 2) · Size: S · Deps: 2
- [ ] **Task 4: Create the repo and push `main`.**
  - Run `git remote rename origin upstream`, then `gh repo create kobywelman99-tech/WhimprFlow --public --description "Local-first voice dictation for macOS (fork of Blueturboguy07/WhimprFlow)"`, then `git remote add origin <url>`.
  - Run `git branch -m ios-app main`, then `git push -u origin main`. Set the default branch to `main`.
  - *Accept:* `gh repo view kobywelman99-tech/WhimprFlow --json visibility,defaultBranchRef` returns PUBLIC/main. `git rev-list --count origin/main` matches local. No other branches or tags on the remote.
  - *Verify:* open the repo URL logged out, or in a private window, and check the README and LICENSE show.
  - Size: XS · Deps: 3

### Checkpoint B: the code is public and correct
- [ ] Koby has looked at the repo page and is happy with what's public

### Phase 3: Something the friend can download
- [ ] **Task 5: Release v0.3.0 with the dmg.** Rebuild from the pushed `main` commit, so the dmg matches the tag. Run `gh release create v0.3.0 <dmg> --title "WhimprFlow 0.3.0" --notes-file <notes>`. The notes cover what's new (Parakeet, in-app model downloads, VAD, latency insights), the Open Anyway steps, and that it's not notarized.
  - *Accept:* `gh release view v0.3.0` lists the dmg. The `releases/latest` URL resolves to v0.3.0.
  - Size: XS · Deps: 4
- [ ] **Task 6: Test the friend's path and write the message for the friend.** Download the dmg from the release URL in a browser, so macOS adds quarantine like it would for the friend. Install it to `/Applications` after moving any existing build aside, and walk through Open Anyway, permissions, model download and one dictation. Then draft a short message for the friend with the link and the 5 steps.
  - *Accept:* a fresh download installs and dictates by following only the README steps.
  - Size: XS · Deps: 5

### Checkpoint C: done
- [ ] Release link works, install steps proven, friend message ready

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| x86_64 cross-compile fails (whisper.cpp, llama.cpp or onnxruntime native libs) | Med | Done in Task 2, before anything is public. Fallback: separate arm64 and x86_64 dmgs, or arm64 only with a note |
| macOS 15+ removed the right-click → Open bypass | Med | Document the System Settings → **Open Anyway** route, which works on 14 and 15+ |
| An ad-hoc signature changes every build, so Accessibility shows "granted" but doesn't work after an update | Low | Already covered in the README troubleshooting (remove and re-add). Repeat it in the release notes |
| Same bundle id `com.whimpr.whimprflow` as the original | Low | The friend shouldn't have both installed. Mention it in the notes |
| Building or pushing while conflict markers remain | High | Task 1 gate: grep for markers before anything else |
| `publik-patch-agent.yml` refers to someone else's org secrets | Low | Only runs on manual dispatch and does nothing without secrets. Left as is |

## Open Questions
- None blocking. The friend's Mac type is unknown, and the universal build covers both.
