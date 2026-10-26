# Todo: Publish to kobywelman99-tech/WhimprFlow

Plan: [plan.md](plan.md). The iOS work has its own list in [../todo.md](../todo.md).

## Phase 1: Local foundation
- [x] Task 1: Clean `ios-app` (merge aborted, no conflict markers, HEAD `2e5bc29`, noreply email set)
- [ ] Task 2: `--adhoc` flag in `scripts/build-macos.sh`, and an Apple Silicon build works
  - [x] `--adhoc` flag added
  - [x] Fixed `beforeBuildCommand`/`beforeDevCommand`: they ran `pnpm build` from the repo root, which has no package.json (upstream's fix: `pnpm --dir ui run …`)
  - [x] Universal attempted: **x86_64 fails**, `ort-sys` has no prebuilt ONNX Runtime for Intel Macs. Fallback taken: arm64 only
  - [x] dmg in `target/aarch64-apple-darwin/release/bundle/dmg/` (15.7 MB). Tauri's `bundle_dmg.sh` failed at the Finder-styling step, so the dmg was made with `hdiutil` instead (same as the script's notarize branch)
  - [x] `lipo -archs` → `arm64`
  - [x] `codesign -dv` → `Signature=adhoc`, hardened runtime on
  - [x] `verify-macos.sh`: the app starts and stays running. The only FAIL is "no signing authority", which is expected for ad-hoc. Gatekeeper rejects a quarantined copy, which is expected and why the friend uses Open Anyway
  - [ ] The app downloads a model and dictates (Koby, manual)

## Checkpoint A
- [x] `cargo test --workspace` passes (295 passed, 0 failed)
- [ ] Koby confirms dictation works in the built app

## Phase 2: Make the repo his
- [x] Task 3: README install section, upstream credit, model text fixed, version 0.3.0 (`Cargo.toml` workspace, `tauri.conf.json`, `ui/package.json`, `Cargo.lock`). README states Apple Silicon only and that the offline cleanup worker isn't in the download yet
- [ ] Task 4: `origin` → `upstream`, create the public repo, push `ios-app` as `main`

## Checkpoint B
- [ ] Koby has reviewed the public repo page

## Phase 3: Downloadable
- [ ] Task 5: Release v0.3.0 with the dmg and notes
- [ ] Task 6: Fresh download installs and dictates, friend message drafted

## Checkpoint C
- [ ] Release link works, install steps proven, friend message ready
