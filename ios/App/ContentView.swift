import SwiftUI

struct ContentView: View {
    let dictation: DictationService
    let flow: FlowController
    let settings: CleanupSettings
    let cleanup: CleanupPipeline
    let dictionary: DictionaryModel
    let history: DictationHistory
    let style: StyleModel
    let setup: SetupModel
    @State private var testText = ""
    @State private var showingSettings = false
    @State private var showingSetup = false
    /// Setup opens by itself once per launch until the keyboard works.
    @State private var offeredSetup = false
    @State private var importNotice: String?
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        NavigationStack {
            List {
                if setup.buildExpiresSoon(), let expires = setup.buildExpires {
                    Section {
                        Label("This test build stops opening \(expires.formatted(.relative(presentation: .named))). Plug the iPhone into the Mac and run ios/dev.sh phone to renew it.", systemImage: "calendar.badge.exclamationmark")
                            .foregroundStyle(.orange)
                    }
                }
                if flow.isOn {
                    Section {
                        FlowBanner(flow: flow)
                    }
                } else if let error = flow.lastError {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }
                Section("Dictate") {
                    DictationPanel(model: dictation)
                }
                Section {
                    NavigationLink {
                        DictionaryView(model: dictionary)
                    } label: {
                        LabeledContent {
                            Text(dictionary.entries.count == 1 ? "1 word" : "\(dictionary.entries.count) words")
                        } label: {
                            Label("Dictionary", systemImage: "character.book.closed")
                        }
                    }
                    NavigationLink {
                        StyleView(model: style)
                    } label: {
                        LabeledContent {
                            Text(style.overview.ready ? style.overview.toneLabel : "\(style.overview.samples) of \(style.overview.minSamplesToApply)")
                        } label: {
                            Label("Your style", systemImage: "person.text.rectangle")
                        }
                    }
                    NavigationLink {
                        HistoryView(history: history)
                    } label: {
                        LabeledContent {
                            Text("\(history.records.count)")
                        } label: {
                            Label("History", systemImage: "clock.arrow.circlepath")
                        }
                    }
                } footer: {
                    Text("Names and terms in your dictionary are spelled right when you dictate. History keeps every dictation on this iPhone.")
                }
                Section {
                    Button {
                        showingSetup = true
                    } label: {
                        LabeledContent {
                            Text("\(setup.snapshot.doneCount) of \(SetupStep.allCases.count)")
                        } label: {
                            Label(setup.snapshot.isReady ? "Setup" : "Finish setting up",
                                  systemImage: setup.snapshot.isReady ? "checkmark.circle" : "exclamationmark.circle")
                                .foregroundStyle(setup.snapshot.isReady ? Color.primary : Color.accentColor)
                        }
                    }
                }
                Section("Try it here") {
                    TextField("Tap here, then switch keyboards", text: $testText, axis: .vertical)
                        .lineLimit(3...6)
                }
            }
            .navigationTitle("WhimprFlow")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Settings", systemImage: "gearshape") { showingSettings = true }
                }
            }
            .sheet(isPresented: $showingSettings, onDismiss: setup.refresh) {
                SettingsView(settings: settings, pipeline: cleanup)
            }
            .sheet(isPresented: $showingSetup, onDismiss: setup.refresh) {
                SetupView(model: setup, settings: settings, cleanup: cleanup)
            }
            .onAppear {
                setup.refresh()
                learnFromImportFolder()
                // Not when the keyboard just opened the app to start Flow:
                // the Flow banner is what matters then.
                if !offeredSetup, !setup.snapshot.isReady, !flow.isOn {
                    offeredSetup = true
                    showingSetup = true
                }
            }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active {
                    setup.refresh()
                    learnFromImportFolder()
                }
            }
            .alert(importNotice ?? "", isPresented: Binding(get: { importNotice != nil }, set: { if !$0 { importNotice = nil } })) {
                Button("OK") { importNotice = nil }
            }
        }
    }
}

extension ContentView {
    /// Writing copied into Documents/Import over the cable (the email corpus).
    fileprivate func learnFromImportFolder() {
        do {
            let learned = try StyleImport.learnFromImportFolder(into: style)
            if learned > 0 { importNotice = "Learned your style from \(learned) pieces of writing." }
        } catch {
            importNotice = error.localizedDescription
        }
    }
}

/// Shown after the keyboard opens the app to start Flow.
private struct FlowBanner: View {
    let flow: FlowController

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Flow is on", systemImage: "waveform")
                .font(.headline)
                .foregroundStyle(.green)
            Text("Go back to your app with ◀ in the top-left corner, then tap the mic on the WhimprFlow keyboard to dictate.")
            Text("Flow stays on until you go 15 minutes without dictating.")
                .font(.footnote)
                .foregroundStyle(.secondary)
            Button("End Flow", role: .destructive) { flow.end() }
                .font(.subheadline.weight(.semibold))
        }
        .padding(.vertical, 4)
    }
}

private struct DictationPanel: View {
    let model: DictationService

    var body: some View {
        VStack(spacing: 14) {
            Button {
                Task { await model.toggleInApp() }
            } label: {
                Image(systemName: model.phase == .recording ? "stop.fill" : "mic.fill")
                    .font(.system(size: 30, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 76, height: 76)
                    .background(model.phase == .recording ? Color.red : Color.accentColor, in: Circle())
            }
            .buttonStyle(.plain)
            .disabled(isBusy)
            .accessibilityLabel(model.phase == .recording ? "Stop dictating" : "Start dictating")

            Text(statusText)
                .font(.footnote)
                .foregroundStyle(isError ? Color.red : Color.secondary)
                .multilineTextAlignment(.center)

            if !model.transcript.isEmpty {
                Text(model.transcript)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
                if let cleanup = model.lastCleanup, model.phase == .idle {
                    if cleanup.cleaned {
                        Label("Polished by Claude", systemImage: "sparkles")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else if let note = cleanup.note {
                        Label(note, systemImage: "info.circle")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Button("Copy", systemImage: "doc.on.doc") {
                    UIPasteboard.general.string = model.transcript
                }
                .font(.footnote)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
    }

    private var isBusy: Bool {
        switch model.phase {
        case .preparing, .transcribing, .cleaning: true
        default: false
        }
    }

    private var isError: Bool {
        if case .failed = model.phase { return true }
        return false
    }

    private var statusText: String {
        switch model.phase {
        case .idle:
            if let latency = model.lastLatency {
                return "Done in \(latency.formatted(.units(allowed: [.seconds, .milliseconds], width: .narrow))). Tap to dictate again."
            }
            return "Tap to dictate"
        case .preparing(let message): return message
        case .recording: return "Listening… tap to stop"
        case .transcribing: return "Finishing…"
        case .cleaning: return "Polishing with Claude…"
        case .failed(let message): return message
        }
    }
}

#Preview {
    let settings = CleanupSettings()
    let cleanup = CleanupPipeline(settings: settings)
    let dictation = DictationService(cleanup: cleanup)
    ContentView(dictation: dictation, flow: FlowController(dictation: dictation, settings: settings), settings: settings, cleanup: cleanup, dictionary: DictionaryModel(), history: DictationHistory(), style: StyleModel(), setup: SetupModel(settings: settings))
}
