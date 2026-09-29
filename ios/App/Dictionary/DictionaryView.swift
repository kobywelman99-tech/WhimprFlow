import SwiftUI

struct DictionaryView: View {
    let model: DictionaryModel

    @State private var word = ""
    @State private var heardAs = ""
    @State private var error: String?
    @FocusState private var focusedField: Field?

    private enum Field { case word, heardAs }

    var body: some View {
        List {
            Section {
                TextField("Word, e.g. WhimprFlow", text: $word)
                    .focused($focusedField, equals: .word)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.next)
                    .onSubmit { focusedField = .heardAs }
                TextField("Also heard as (optional, comma-separated)", text: $heardAs)
                    .focused($focusedField, equals: .heardAs)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.done)
                    .onSubmit(add)
                Button("Add word", action: add)
                    .disabled(word.trimmingCharacters(in: .whitespaces).isEmpty)
                if let error {
                    Text(error).foregroundStyle(.red)
                }
            } header: {
                Text("Add a word")
            } footer: {
                Text("Use the exact spelling you want. If dictation keeps writing it wrong, add what it writes under “Also heard as”.")
            }

            Section(model.entries.isEmpty ? "" : "Your words") {
                if model.entries.isEmpty {
                    Text("No words yet. Add names, places and terms you use often.")
                        .foregroundStyle(.secondary)
                }
                ForEach(model.entries, id: \.correct) { entry in
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(entry.correct)
                            if entry.learned {
                                Image(systemName: "sparkles")
                                    .foregroundStyle(.secondary)
                                    .accessibilityLabel("Learned automatically")
                            }
                        }
                        if !entry.mishears.isEmpty {
                            Text("heard as \(entry.mishears.joined(separator: ", "))")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .swipeActions {
                        Button("Delete", role: .destructive) { remove(entry.correct) }
                    }
                }
            }
        }
        .navigationTitle("Dictionary")
    }

    private func add() {
        do {
            try model.add(word: word, heardAs: heardAs)
            word = ""
            heardAs = ""
            error = nil
            focusedField = .word
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func remove(_ word: String) {
        do {
            try model.remove(word)
        } catch {
            self.error = error.localizedDescription
        }
    }
}
