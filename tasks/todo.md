# TODO: WhimprFlow for iPhone

Design and rationale: `tasks/plan.md`. Target: iPhone 14 Pro on iOS 26.6.1, free Apple ID (Personal Team), Anthropic cleanup.

## Phase 0: Toolchain + "hello iPhone"

### Task 0: Dev environment
**Description:** Set up everything needed to build and sign an iOS app on this Mac and put it on Koby's iPhone.

**Koby (manual):**
- [x] Install Xcode from the App Store, then open it once and add the iOS platform
- [x] Xcode ▸ Settings ▸ Accounts: add Apple ID (creates the Personal Team)
- [x] iPhone: Settings ▸ Privacy & Security ▸ Developer Mode ON (restarts the phone)
- [x] Connect iPhone by cable and tap "Trust This Computer"

**Claude:**
- [x] `brew install xcodegen`
- [x] `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`
- [x] `sudo xcode-select -s /Applications/Xcode.app` (Koby runs this; it needs sudo)

**Acceptance criteria:**
- [x] `xcodebuild -version` prints an Xcode version
- [x] `xcrun devicectl list devices` lists the iPhone
- [x] `security find-identity -v -p codesigning` shows an "Apple Development" identity

**Dependencies:** None · **Scope:** XS (no repo files)

### Task 1: Scaffold host app + keyboard extension
**Description:** XcodeGen project with a SwiftUI host app and a UIKit keyboard extension that can type basic keys. The mic button is a stub.

**Acceptance criteria:**
- [ ] `xcodegen generate` + simulator build succeed
- [ ] Installs on the iPhone via Personal Team
- [ ] Keyboard is listed in Settings ▸ General ▸ Keyboards and types letters, space, delete and return into Notes; globe switches keyboards

**Verification:** `ios/dev.sh test` (keyboard layout tests on the simulator); `ios/dev.sh phone` runs the tests and only installs if they pass; manual on-device check

**Progress:**
- [x] Builds for the simulator and installs on the iPhone (team in gitignored `ios/Config/Local.xcconfig`)
- [x] First on-device run crashed on launch (layout constraint activated before its row was in the view hierarchy). Fixed; `KeyboardLayoutTests` reproduces it and also checks that no key is cut off
- [x] Koby confirms on the phone: keyboard stays up and types; mic stub shows its placeholder

**Dependencies:** T0 · **Files:** `ios/project.yml`, `ios/App/WhimprFlowApp.swift`, `ios/App/Info.plist`, `ios/Keyboard/KeyboardViewController.swift`, `ios/Keyboard/Info.plist` · **Scope:** S

## Checkpoint 0
- [ ] App and keyboard are on the phone and the keyboard types
- [ ] Review with Koby

## Phase 1: De-risk spikes

### Task 2: Foreground dictation in the host app
**Description:** AVAudioEngine capture + SpeechAnalyzer/SpeechTranscriber (en-US asset via AssetInventory). Push-to-talk button shows the transcript.

**Acceptance criteria:**
- [ ] Accurate English transcript of a 10 s utterance
- [ ] Latency from stop to text logged via os_log (target < 1 s on the 14 Pro)
- [ ] Mic permission denial shows a clear message

**Verification:** simulator build + unit test for the audio-format conversion; on-device manual test

**Progress:**
- [x] Recorder + BufferConverter (flushes the resampler tail on stop so the last word isn't clipped), SpeechEngine, DictationModel, Dictate panel
- [x] `BufferConverterTests` (simulator) and `SpeechEngineTests` (the real model, run on the iPhone: `xcodebuild test -destination id=<udid> -only-testing:WhimprFlowTests/SpeechEngineTests`; the simulator has no SpeechTranscriber, so it skips there) pass
- [x] Koby dictates in the app on the phone: works

**Dependencies:** T1 · **Files:** `ios/App/Audio/Recorder.swift`, `ios/App/ASR/SpeechEngine.swift`, `ios/App/ContentView.swift` · **Scope:** S

### Task 3: Background Flow Session (riskiest)
**Description:** `whimprflow://flow` starts a session: audio session active, engine kept alive in background, auto-ends after N minutes. Darwin notifications `start`/`stop` control capture; transcription happens while the app is backgrounded.

**Acceptance criteria:**
- [ ] With the app backgrounded and Notes in front, Darwin start/stop produces a transcript in the app log
- [ ] Session survives 5 minutes idle; the mic indicator shows only while the session is live
- [ ] If SpeechTranscriber fails in the background, record it and switch T2's engine to WhisperKit before continuing

**Verification:** on-device test with Console.app log stream (subsystem `com.whimpr.whimprflow`)

**Progress:**
- [x] Built: Recorder session/capture split (mic runs for the whole session; iOS won't start it from the background), DictationService, FlowController (15 min idle timeout, ends on audio interruption), Flow banner in the app
- [x] On the phone: dictation from the keyboard works while WhimprFlow is in the background

**Dependencies:** T2 · **Files:** `ios/App/Flow/FlowSession.swift`, `ios/Shared/Bridge/DarwinSignals.swift`, `ios/App/WhimprFlowApp.swift` · **Scope:** M

### Task 4: Bridge the result into the text field (no App Groups)
**Description:** `DictationBridge` protocol. Keyboard opens `whimprflow://flow` (responder-chain openURL) and sends start/stop over Darwin notifications. The result comes back over a loopback socket (A) or UIPasteboard + Darwin "done" (B). Keyboard inserts it via `textDocumentProxy.insertText`.

**Acceptance criteria:**
- [ ] In Notes and in Messages: keyboard mic → speak → stop → raw text inserted at the cursor with no manual paste
- [ ] If the pasteboard route is used, the user's previous clipboard is restored
- [x] Chosen transport and the reason for it recorded in `tasks/plan.md`

**Progress:**
- [x] Built: loopback bridge (`Shared/BridgeProtocol.swift`, `App/Flow/BridgeServer.swift`, `Keyboard/BridgeClient.swift`), `AppOpener` (keyboard opens `whimprflow://flow?token=…`), keyboard mic states and text insertion
- [x] `BridgeTests`: real client↔server over loopback in the simulator (token enforced, both directions, disconnects)
- [x] On the phone: keyboard mic → speak → text inserted (confirmed by Koby)

**Verification:** on-device manual test in 2+ apps

**Dependencies:** T3 · **Files:** `ios/Shared/Bridge/DictationBridge.swift`, `ios/Shared/Bridge/LoopbackBridge.swift` or `PasteboardBridge.swift`, `ios/Keyboard/KeyboardViewController.swift`, `ios/App/Flow/FlowSession.swift` · **Scope:** M

## Checkpoint 1
- [x] Raw end-to-end dictation from the keyboard works in 2+ apps
- [x] Review with Koby

## Phase 2: Rust core + cleanup

### Task 5: `whimpr-ffi` UniFFI crate + XCFramework
**Description:** Expose whimpr-core's cleanup (build_messages, pre_normalize_layout, gates, post_process), DictionaryStore, StyleStore, StatsStore and Settings to Swift. `scripts/build-ios.sh` builds `WhimprCore.xcframework` + Swift bindings. The host app links it; the keyboard does not.

**Acceptance criteria:**
- [ ] `cargo test -p whimpr-ffi` passes; `cargo check --workspace` still passes (desktop unaffected)
- [ ] Host app gets the same system prompt from `buildMessages` as desktop does for the same input
- [ ] Keyboard extension binary size unchanged

**Verification:** cargo tests + a Swift unit test calling into Rust on the simulator

**Progress:**
- [x] `crates/whimpr-ffi`: `prepare_cleanup` / `finish_cleanup` wrap the desktop pipeline (hotkey.rs `clean_transcript`) around the network call; `Level::Off` (not `None`, which Swift reads as nil)
- [x] `cargo test -p whimpr-ffi`: 8 pass, including byte-identical prompt vs desktop; `cargo check --workspace` passes
- [x] `scripts/build-ios.sh` → `ios/Frameworks/WhimprCore.xcframework` + `ios/Generated/` (gitignored, rebuilt by `ios/dev.sh`)
- [x] `RustCoreTests` (5) pass in the simulator and on the iPhone; keyboard binary has no Rust symbols

**Dependencies:** T1 (can run in parallel with T2–T4) · **Files:** `crates/whimpr-ffi/Cargo.toml`, `crates/whimpr-ffi/src/lib.rs`, `Cargo.toml`, `scripts/build-ios.sh`, `ios/project.yml` · **Scope:** M

### Task 6: Anthropic cloud cleanup
**Description:** URLSession Anthropic Messages client (`claude-haiku-4-5`, mirroring `AnthropicProvider` in `crates/whimpr-cleanup`). API key in the Keychain. Settings screen for key, cleanup level and a Raw toggle. Pipeline: transcript → Rust pre-normalize → build_messages → Anthropic → Rust gates/post_process → bridge.

**Acceptance criteria:**
- [ ] Filler words and punctuation are cleaned in the inserted text
- [ ] In airplane mode, raw text is inserted within the timeout
- [ ] The API key never appears in logs or on disk outside the Keychain

**Verification:** unit test for request building and response parsing (canned JSON); on-device manual test

**Progress:**
- [x] `AnthropicClient` (raw HTTPS, no Swift SDK; same request as desktop `AnthropicProvider`; 8 s timeout, one retry on 429/5xx/network; cut-off and refusal answers rejected), `KeychainStore`, `CleanupSettings`, `CleanupPipeline`, Settings screen with "Test cleanup"
- [x] Keyboard shows "Polishing…" (new `cleaning` bridge message)
- [x] Tests: `AnthropicClientTests` (10, stubbed network, mutation-checked), `CleanupPipelineTests` (7), `KeychainStoreTests`; 29 app tests pass on the iPhone
- [ ] Koby: add key in Settings, Test cleanup, then dictate from the keyboard

**Dependencies:** T4, T5 · **Files:** `ios/App/Cleanup/CloudCleanup.swift`, `ios/App/Cleanup/KeychainStore.swift`, `ios/App/Settings/SettingsView.swift`, `ios/App/Flow/FlowSession.swift` · **Scope:** M

### Task 7: Dictionary
**Description:** Dictionary screen backed by Rust `DictionaryStore`. Terms are fed to SpeechAnalyzer as contextual strings and to the cleanup prompt via `prefilter`.

**Acceptance criteria:**
- [ ] A custom name that is misspelled by default comes out right after it's added
- [ ] Entries persist across relaunch

**Verification:** Swift unit test for the store round-trip; on-device manual test

**Progress:**
- [x] Rust `UserDictionary` (whimpr-ffi) wraps whimpr-core's `DictionaryStore`: same JSON format as desktop `dictionary.json`, prefilter for the prompt (max 15). 14 Rust tests
- [x] Words reach the cleanup prompt (`DictionaryCleanupTests`) and the recognizer as vocabulary hints
- [x] **Recognizer switched to DictationTranscriber.** SpeechTranscriber ignored hints on the iPhone ("Kobe Wellman… Zyfrex" with or without them); DictationTranscriber gets "Koby Welman… Zyphrex". `SpeechEngineTests.testVocabularyFixesUnusualNames` checks this on the phone
- [x] Dictionary screen (add with "also heard as", swipe to delete); readable errors (the generated Swift errors print their case name, caught by a test)
- [x] 36 app tests pass on the iPhone; simulator suite passes; `cargo check --workspace` passes
- [ ] Koby adds real words and dictates them from the keyboard

**Dependencies:** T5, T6 · **Files:** `ios/App/Dictionary/DictionaryView.swift`, `ios/App/ASR/SpeechEngine.swift`, `ios/App/Flow/FlowSession.swift` · **Scope:** S/M

## Checkpoint 2
- [ ] Cleaned, dictionary-aware dictation works from the keyboard
- [ ] Koby tries it in daily use for a few days

## Phase 3: Polish

### Task 8: Keyboard UX
Chosen with Koby (2026-09-29), from user feedback on Wispr-style keyboards and the Dictus keyboard (MIT; design ideas only so far): full QWERTY with a mic bar, a style pill for **cleanup levels**, a recording overlay, an Undo chip. Tap to toggle (no hold-to-talk this round).

#### 8a: Bridge carries level override and live audio levels (M)
- [x] `start` carries the pill's level; `ready` carries the app's default level; app → keyboard `level` messages (~15/s) while recording
- [x] Recorder meters the mic (RMS → 0…1); CleanupPipeline takes a per-dictation level override
- [x] Tests: protocol round trip (old messages still decode), metering of known signals, override reaches the prompt, bridge client↔server with the new fields
**Files:** `Shared/BridgeProtocol.swift`, `App/Audio/Recorder.swift`, `App/Cleanup/CleanupPipeline.swift`, `App/Dictation/DictationService.swift`, `App/Flow/FlowController.swift`, `Keyboard/BridgeClient.swift`

#### 8b: Mic bar: style pill, mic pill, status, haptics, VoiceOver (M)
- [x] 52pt bar: `[Light ▾]` pill (tap cycles Off → Light → Medium → High, starts at the app default each time the keyboard opens), status text, 56×36 mic pill (outlined when Flow is off, filled when on)
- [x] Haptics on start / stop / inserted / error; VoiceOver labels and values on every control
- [x] Tests: layout still fits at 320/393pt, pill cycling, mic states; snapshot light and dark
**Files:** `Keyboard/KeyboardViewController.swift`, `Keyboard/MicBar.swift`, `Tests/KeyboardTests/*`

#### 8c: Recording overlay (M)
- [x] Covers the keys while recording: ✕ cancel and timer; live waveform (blue) from `audioLevel` messages; what's been heard so far; grey sweep while "Finishing…" / "Polishing…". **Changed after visual review:** no ✓ button or level label in the overlay (they duplicated the bar's stop button and pill), so stop = the bar's mic, under the same thumb
- [x] Tests: overlay appears on `recording`, hides on `final` / `error` / `noSession`; ✕ sends `cancel`, ✓ sends `stop`; waveform keeps a fixed number of bars; snapshot light and dark
**Files:** `Keyboard/RecordingOverlay.swift`, `Keyboard/WaveformView.swift`, `Keyboard/KeyboardViewController.swift`

#### 8d: Undo chip (S)
- [x] After inserting, "Undo" appears in the bar for 6 s; tapping it removes exactly the inserted text, only if it's still right before the cursor
- [x] Tests: undo removes the insert; does nothing if the user typed after it; expires
**Files:** `Keyboard/UndoTracker.swift`, `Keyboard/KeyboardViewController.swift`

#### Checkpoint 8
- [x] All tests pass: simulator, and 72 on the iPhone (30 keyboard + 42 app)
- [ ] Koby tries the new keyboard in WhatsApp/Messages

#### Bugs from Koby's first day of real use (2026-09-29)
- [x] **Music and headphones while Flow is on.** Fixed: `Recorder.sessionOptions` adds `.allowBluetoothA2DP`, and a route change now restarts the mic instead of ending Flow (the second bug, found while fixing the first); a mid-dictation format change swaps the converter without losing audio. Tests: `AudioSessionTests`, `RouteChangeTests` (mutation-checked). 85 tests pass on the iPhone. [ ] Koby checks with his headphones.
  Original report: Music moves to the phone speaker and Bluetooth headphones don't connect. Likely cause: the session is `.playAndRecord` with `[.mixWithOthers, .defaultToSpeaker]` and no `.allowBluetoothA2DP`, so A2DP output isn't allowed. Fix: add `.allowBluetoothA2DP`, check that the mic stays built-in and output goes to headphones. Needs Koby's headphones to verify
- [x] **Scrolling loses the dictation.** Fixed (2026-09-29): the keyboard no longer cancels when it closes; the app finishes the dictation, and results only go to the keyboard connection that started them (numbered connections). One that finishes after its keyboard closed is kept for 10 minutes and offered as an "Insert '…'" chip the next time the keyboard opens (tap to insert, then Undo). Tests: `KeyboardUXTests` (mutation-checked against the old cancel), `BridgeTests`, `PendingDictationTests`.
  Original report: Probable cause: scrolling a chat dismisses the keyboard, and `viewWillDisappear` sends `cancel`. Fix: send `stop` instead, then insert when the keyboard comes back, or keep the text for "insert last". Confirm with Koby what he saw

#### Later list (from the same feedback)
- Hold-to-talk in addition to tap · Dynamic Island Live Activity for Flow · Flow timeout setting · Tone styles (Formal / Casual / Very casual / Excited; needs shared Rust prompt work) · **Per-context styles** (text message vs email vs report/essay: the keyboard can't see the host app, so infer from the field (Send return key, email field) plus a context pill)

### Task 9: Onboarding
- [x] Setup checklist (`App/Setup/`), opened by itself once per launch until the keyboard works (not when the keyboard just opened the app to start Flow): microphone → add keyboard → Full Access → Anthropic key (optional) → first dictation
- [x] Steps tick themselves from what the app can see: mic permission; iOS's enabled-keyboards list (undocumented, so "unknown" when absent); the keyboard's first authorized connection (proves Full Access); the first keyboard dictation
- [x] Free-account build expiry read from `embedded.mobileprovision`; shown in Setup, with an orange banner on the main screen in the last 2 days
- [x] Tests: `SetupSnapshotTests` (6), `ProvisioningInfoTests` (the real profile on the phone), `BridgeTests.testAuthorizedFiresOnlyForTheRightToken`; setup screen reviewed as images (finished steps collapse to their title)
- [x] Installed; 82 tests pass on the iPhone (31 keyboard + 51 app). The real profile parses (expires 2026-10-06 UTC), and iOS does share the enabled-keyboards list, which includes `com.whimpr.whimprflow.ios.keyboard`, so step 2 ticks without waiting for a connection

**Dependencies:** T4 · **Scope:** S

### Task 10: Learn Koby's style from everything he says and writes
Decided with Koby (2026-09-29): keep every dictation; learn from all of them (keyboard and in-app); seed from his last 200 sent emails (pulled with the Gmail connector in a Claude session, his own words only) and from in-app imports; phone and desktop profiles stay separate; tones and per-context styles (message / email / report) go on the later list.

#### 10a: Dictation history (M)
- [x] Every dictation saved on the phone: what was said (raw), what was inserted, date, keyboard or app, level. Kept until deleted
- [x] History screen: newest first, copy either version, swipe to delete, clear all
- [x] Tests: saving, reloading, deleting, and a corrupt file doesn't lose the app
**Files:** `App/History/DictationHistory.swift`, `App/History/HistoryView.swift`, `App/Dictation/DictationService.swift`

#### 10b: Style learning (M)
- [x] `whimpr-ffi` wraps whimpr-core's `StyleStore` (the desktop's passive profile: sentence length, contractions, go-to phrases, tone; applies after 5 samples)
- [x] Each dictation's raw words are observed; the profile's summary goes into the cleanup prompt (`style_summary`), as on desktop
- [x] Style screen: what it has learned, an on/off switch, Reset
- [x] Tests: Rust (observe → summary, persistence), Swift (the summary reaches the prompt, off means no summary)
**Files:** `crates/whimpr-ffi/src/lib.rs`, `App/Style/*`, `App/Cleanup/CleanupPipeline.swift`

#### 10c: Import past writing (M)
- [x] In-app Import: paste text or pick text files; split into samples; observed by the profile; the text itself isn't kept
- [x] Import folder: `Documents/Import/*.txt` is learned on launch, then deleted (how the email corpus gets onto the phone over the cable)
- [x] Email corpus: last 200 sent Gmail messages, his own words only (no quoted replies, greetings, sign-offs or signatures, so "Best regards" can't become a "go-to phrase")
- [x] Tests: splitting and stripping, the import folder is learned and emptied
**Files:** `App/Style/StyleImport.swift`, `App/Style/StyleView.swift`

**Done (2026-09-29):** 166 of Koby's last 200 sent emails learned on the phone (Gmail search previews, own words only; 12 empty, 76 previews cut at the last full sentence). The first pass leaked his signature ("koby welman", phone-number fragments) into the phrase counts, so the profile was wiped and re-learned after cleaning; the importer now cuts inline signatures and phone numbers (tests added). Every copy of the email text was deleted from the Mac, and the app deleted its copy after learning. 102 tests pass on the iPhone, 18 Rust.
**Done (desktop core, `style.rs`, left uncommitted with Koby's other desktop work):** `top_phrases` drops fragments that rarely appear outside a longer phrase (two new tests; whimpr-core 54 pass).
**Was:** repeated-phrase counting reports overlapping fragments ("let me", "me know", "let me know"); keep only the longest phrase that covers the others. Not changed here because it's Koby's uncommitted desktop work.

## Checkpoint: v1 complete
- [ ] All acceptance criteria met; `cargo check --workspace` green; review with Koby

## Later (not v1)
- [ ] Local cleanup spike: llama.cpp on CPU with Qwen2.5-0.5B/1.5B Q4
- [ ] Paid-account upgrade: AppGroupBridge + shared Keychain + TestFlight
