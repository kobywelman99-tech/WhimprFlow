import SwiftUI
import UniformTypeIdentifiers

struct StyleView: View {
    let model: StyleModel

    @State private var pasting = false
    @State private var pickingFiles = false
    @State private var confirmingReset = false
    @State private var message: String?

    var body: some View {
        List {
            statusSection
            if model.overview.samples > 0 {
                noticedSection
            }
            importSection
            Section {
                Toggle("Learn from my dictations", isOn: Binding(get: { model.enabled }, set: { model.enabled = $0 }))
                Button("Reset what it has learned", role: .destructive) { confirmingReset = true }
                    .disabled(model.overview.samples == 0)
            } footer: {
                Text("Only counts are kept (sentence length, contractions, phrases you repeat), on this iPhone. When learning is off, cleanup doesn't use your style.")
            }
        }
        .navigationTitle("Your style")
        .sheet(isPresented: $pasting) {
            PasteSheet { text in learn(StyleImport.samples(from: text), what: "the text") }
        }
        .fileImporter(isPresented: $pickingFiles, allowedContentTypes: [.plainText], allowsMultipleSelection: true) { result in
            importFiles(result)
        }
        .confirmationDialog("Forget your style?", isPresented: $confirmingReset, titleVisibility: .visible) {
            Button("Reset", role: .destructive) {
                do { try model.reset() } catch { message = error.localizedDescription }
            }
        } message: {
            Text("Your dictation history stays.")
        }
        .alert(message ?? "", isPresented: Binding(get: { message != nil }, set: { if !$0 { message = nil } })) {
            Button("OK") { message = nil }
        }
    }

    private var statusSection: some View {
        let overview = model.overview
        return Section {
            if overview.ready {
                LabeledContent("Tone", value: overview.toneLabel.capitalized)
                if let summary = model.promptSummary {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("What Claude is told when cleaning up")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Text(summary)
                    }
                    .padding(.vertical, 2)
                }
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Learning: \(overview.samples) of \(overview.minSamplesToApply)")
                        .font(.headline)
                    ProgressView(value: Double(overview.samples), total: Double(overview.minSamplesToApply))
                    Text("After \(overview.minSamplesToApply) dictations or pieces of writing, cleanup starts matching how you write.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 4)
            }
        }
    }

    private var noticedSection: some View {
        let overview = model.overview
        return Section("What it has noticed") {
            LabeledContent("Learned from", value: "\(overview.samples)")
            LabeledContent("Average sentence", value: "\(Int(overview.avgSentenceWords.rounded())) words")
            LabeledContent("Uses contractions", value: "\(Int((overview.contractionRate * 100).rounded()))%")
            if !overview.topPhrases.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Phrases you repeat")
                    Text(overview.topPhrases.map { "“\($0.phrase)”" }.joined(separator: "  "))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
            if !overview.topFillers.isEmpty {
                LabeledContent("Filler words", value: overview.topFillers.map(\.phrase).joined(separator: ", "))
            }
        }
    }

    private var importSection: some View {
        Section {
            Button("Paste some writing", systemImage: "doc.on.clipboard") { pasting = true }
            Button("Choose text files", systemImage: "folder") { pickingFiles = true }
        } header: {
            Text("Learn from your writing")
        } footer: {
            Text("Emails, notes, documents. Greetings, sign-offs and quoted replies are skipped, and the text itself isn't saved.")
        }
    }

    private func learn(_ samples: [String], what: String) {
        do {
            let learned = try model.learn(from: samples)
            message = learned == 0 ? "Nothing to learn from in \(what)." : "Learned from \(learned) piece\(learned == 1 ? "" : "s") of writing."
        } catch {
            message = error.localizedDescription
        }
    }

    private func importFiles(_ result: Result<[URL], Error>) {
        switch result {
        case .success(let urls):
            var samples: [String] = []
            for url in urls {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                if let text = try? String(contentsOf: url, encoding: .utf8) {
                    samples += StyleImport.samples(from: text)
                }
            }
            learn(samples, what: "those files")
        case .failure(let error):
            message = error.localizedDescription
        }
    }
}

private struct PasteSheet: View {
    let onLearn: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""

    var body: some View {
        NavigationStack {
            TextEditor(text: $text)
                .padding(.horizontal)
                .navigationTitle("Paste writing")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Learn") {
                            onLearn(text)
                            dismiss()
                        }
                        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
        }
    }
}
