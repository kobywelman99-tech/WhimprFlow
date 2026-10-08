import SwiftUI

struct SettingsView: View {
    let settings: CleanupSettings
    let pipeline: CleanupPipeline

    @Environment(\.dismiss) private var dismiss
    @State private var keyDraft = ""
    @State private var keyError: String?
    @State private var testing = false
    @State private var testResult: CleanupResult?

    private static let sample = "um so this is uh a quick test of the cleanup new line thanks"

    var body: some View {
        NavigationStack {
            Form {
                cleanupSection
                reviewSection
                keySection
                testSection
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private var cleanupSection: some View {
        Section {
            Toggle("Clean up with Claude", isOn: Binding(get: { settings.enabled }, set: { settings.enabled = $0 }))
            Picker("Level", selection: Binding(get: { settings.level }, set: { settings.level = $0 })) {
                Text("Off").tag(Level.off)
                Text("Light").tag(Level.light)
                Text("Medium").tag(Level.medium)
                Text("High").tag(Level.high)
            }
            .disabled(!settings.enabled)
        } header: {
            Text("Cleanup")
        } footer: {
            Text(levelDescription)
        }
    }

    private var reviewSection: some View {
        Section {
            Toggle("Check it makes sense", isOn: Binding(get: { settings.reviewEnabled }, set: { settings.reviewEnabled = $0 }))
                .disabled(!settings.enabled)
        } header: {
            Text("Sense check")
        } footer: {
            Text("Before a dictation is inserted, Claude re-reads it in the conversation it's going into and fixes any word that doesn't fit, using what you actually said. It only changes a few words and never rewrites. The text before your cursor is sent to Anthropic for this. Skipped at the Raw level.")
        }
    }

    private var levelDescription: String {
        guard settings.enabled else { return "Your words are inserted exactly as heard." }
        return switch settings.level {
        case .off: "Your words are inserted exactly as heard."
        case .light: "Removes filler words and fixes grammar, keeping your wording. Recommended."
        case .medium: "Also tightens wording for clarity, without changing the meaning."
        case .high: "Rewrites for brevity and polish, keeping every fact, name and number."
        }
    }

    private var keySection: some View {
        Section {
            if settings.hasAPIKey {
                Label("Key saved", systemImage: "checkmark.seal.fill")
                    .foregroundStyle(.green)
                Button("Remove key", role: .destructive) {
                    do {
                        try settings.removeAPIKey()
                        testResult = nil
                    } catch {
                        keyError = error.localizedDescription
                    }
                }
            } else {
                SecureField("sk-ant-…", text: $keyDraft)
                    .textContentType(.password)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button("Save key") {
                    do {
                        try settings.saveAPIKey(keyDraft)
                        keyDraft = ""
                        keyError = nil
                    } catch {
                        keyError = error.localizedDescription
                    }
                }
                .disabled(keyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            if let keyError {
                Text(keyError).foregroundStyle(.red)
            }
        } header: {
            Text("Anthropic API key")
        } footer: {
            Text("Kept in this iPhone's Keychain. Only the text you dictate is sent to Anthropic, and only while cleanup is on.")
        }
    }

    private var testSection: some View {
        Section {
            Button {
                Task {
                    testing = true
                    testResult = await pipeline.clean(Self.sample)
                    testing = false
                }
            } label: {
                HStack {
                    Text("Test cleanup")
                    if testing {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .disabled(testing || !settings.enabled || !settings.hasAPIKey)

            if let testResult {
                VStack(alignment: .leading, spacing: 6) {
                    Label(
                        testResult.cleaned ? "Claude cleaned it up" : "Not cleaned up",
                        systemImage: testResult.cleaned ? "checkmark.circle.fill" : "exclamationmark.triangle.fill"
                    )
                    .foregroundStyle(testResult.cleaned ? .green : .orange)
                    Text(testResult.text)
                    if let note = testResult.note {
                        Text(note).font(.footnote).foregroundStyle(.secondary)
                    }
                }
            }
        } header: {
            Text("Check it works")
        } footer: {
            Text("Sends “\(Self.sample)” to Claude.")
        }
    }
}
