import SwiftUI

/// What WhimprFlow has learned about your voice, and the way to teach it more.
struct BrainView: View {
    let brain: BrainModel
    let dictation: DictationService
    @State private var confirmingReset = false
    @State private var error: String?

    var body: some View {
        List {
            Section {
                LabeledContent("Learned from", value: brain.overview.observations == 1 ? "1 sentence" : "\(brain.overview.observations) sentences")
                LabeledContent("Fixed automatically", value: brain.overview.learnedWords == 1 ? "1 word" : "\(brain.overview.learnedWords) words")
                NavigationLink {
                    CalibrationView(model: CalibrationModel(recorder: CalibrationRecorder(dictation: dictation), brain: brain))
                } label: {
                    Label("Calibrate your voice", systemImage: "waveform.badge.mic")
                }
            } footer: {
                Text("It also learns whenever you correct a dictation in History.")
            }

            Section {
                if brain.overview.patterns.isEmpty {
                    Text(brain.overview.observations == 0
                         ? "Nothing yet. Calibrate, or correct a dictation in History, and it will start to notice how you sound."
                         : "No pattern stands out yet. More sentences help it tell habits from one-off slips.")
                        .foregroundStyle(.secondary)
                }
                ForEach(brain.overview.patterns, id: \.meant) { pattern in
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(pattern.meant) → \(pattern.heard)")
                        Text("\(pattern.count) of \(pattern.opportunities) times · \(Int((pattern.rate * 100).rounded()))%")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            } header: {
                Text("Sounds it has noticed")
            } footer: {
                Text("These are the sounds that most often come out differently. They carry over to words it has never seen you say.")
            }

            if let error {
                Section { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red) }
            }

            if brain.overview.observations > 0 {
                Section {
                    Button("Forget everything it learned", role: .destructive) { confirmingReset = true }
                }
            }
        }
        .navigationTitle("Your voice")
        .confirmationDialog("Forget what it learned about your voice?", isPresented: $confirmingReset, titleVisibility: .visible) {
            Button("Forget", role: .destructive) {
                do { try brain.reset() } catch { self.error = error.localizedDescription }
            }
        } message: {
            Text("Your dictionary and writing style stay.")
        }
    }
}
