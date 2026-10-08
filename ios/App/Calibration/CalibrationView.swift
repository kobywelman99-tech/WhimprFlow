import SwiftUI

/// Read some lines aloud so WhimprFlow can learn where it mishears you.
struct CalibrationView: View {
    @State var model: CalibrationModel
    @State private var own = ""

    init(model: CalibrationModel) {
        _model = State(initialValue: model)
    }

    var body: some View {
        Form {
            switch model.step {
            case .intro: intro
            case .reading: reading
            case .learned(let count): learned(count)
            }
            if let error = model.error {
                Section { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red) }
            }
        }
        .navigationTitle("Calibrate your voice")
        .navigationBarTitleDisplayMode(.inline)
        .onDisappear { model.leave() }
    }

    private var intro: some View {
        Section {
            Text("You'll read about \(CalibrationModel.defaultLineCount) short lines aloud, from novels, poems, speeches and everyday talk, in your normal voice. WhimprFlow compares what you said with what it heard, and learns which of your sounds come out differently.")
            Text("It doesn't retrain the speech recogniser. It learns your sound patterns, like a th that comes out as f, and uses them to pick the right words, including words you haven't practised.")
                .font(.footnote)
                .foregroundStyle(.secondary)
            Button("Start") { model.begin() }
                .buttonStyle(.borderedProminent)
        }
    }

    @ViewBuilder
    private var reading: some View {
        if let line = model.currentLine {
            Section {
                Text(line.text)
                    .font(.title3)
                if let heard = line.heard {
                    LabeledContent("Heard") { Text(heard.isEmpty ? "Nothing" : heard).multilineTextAlignment(.trailing) }
                        .font(.callout)
                }
            } header: {
                Text("Line \(model.current + 1) of \(model.lines.count) · \(line.tag)")
            }
            Section {
                micButton(for: line)
                if line.heard != nil {
                    Button(model.isLastLine ? "Finish" : "Next line") { model.next() }
                    Button("Say it again") { model.redo() }
                } else {
                    Button("Skip this line", role: .cancel) { model.skip() }
                }
            }
            Section {
                HStack {
                    TextField("A name or word you want to add", text: $own)
                        .textInputAutocapitalization(.never)
                    Button("Add") {
                        model.addOwn(own)
                        own = ""
                    }
                    .disabled(own.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: {
                Text("Your own words")
            } footer: {
                Text("They join the end of the list. Say the word on its own.")
            }
            Section {
                Button("Done early") { model.finishReading() }
                    .disabled(model.spokenCount == 0)
            }
        }
    }

    private func micButton(for line: CalibrationModel.Line) -> some View {
        Button {
            Task {
                if model.listening == .listening {
                    await model.stopListening()
                } else {
                    await model.startListening()
                }
            }
        } label: {
            switch model.listening {
            case .idle: Label(line.heard == nil ? "Tap, then read the line" : "Read again", systemImage: "mic.fill")
            case .listening: Label("Listening… tap when done", systemImage: "stop.circle.fill")
            case .working: Label("Working…", systemImage: "hourglass")
            }
        }
        .disabled(model.listening == .working || line.heard != nil)
    }

    private func learned(_ count: Int) -> some View {
        Section {
            Label("Learned from \(count) \(count == 1 ? "line" : "lines").", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
            Text("See what it noticed on the Your voice screen. Run this again any time; each run teaches it more.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }
}
