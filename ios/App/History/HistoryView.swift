import SwiftUI

struct HistoryView: View {
    let history: DictationHistory
    let brain: BrainModel
    @State private var confirmingClear = false

    var body: some View {
        List {
            if history.records.isEmpty {
                Text("Your dictations will appear here.")
                    .foregroundStyle(.secondary)
            }
            ForEach(history.records) { record in
                NavigationLink {
                    RecordDetail(record: record, brain: brain)
                } label: {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(record.text).lineLimit(3)
                        Text(caption(for: record))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .swipeActions {
                    Button("Delete", role: .destructive) { history.delete(record.id) }
                }
            }
        }
        .navigationTitle("History")
        .toolbar {
            if !history.records.isEmpty {
                Button("Clear", role: .destructive) { confirmingClear = true }
            }
        }
        .confirmationDialog("Delete all \(history.records.count) dictations?", isPresented: $confirmingClear, titleVisibility: .visible) {
            Button("Delete All", role: .destructive) { history.clear() }
        } message: {
            Text("What WhimprFlow has learned about your style stays.")
        }
    }

    private func caption(for record: DictationRecord) -> String {
        let when = record.date.formatted(date: .abbreviated, time: .shortened)
        let from = record.source == .keyboard ? "Keyboard" : "App"
        let how = record.cleaned ? (record.level?.title ?? "Cleaned") : "Your words"
        return "\(when) · \(from) · \(how)"
    }
}

private struct RecordDetail: View {
    let record: DictationRecord
    let brain: BrainModel
    @State private var correcting = false
    @State private var corrected = ""
    @State private var notice: String?

    var body: some View {
        List {
            Section("Inserted") {
                Text(record.text).textSelection(.enabled)
                Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = record.text }
            }
            if let fixes = record.fixes, !fixes.isEmpty {
                Section {
                    ForEach(fixes, id: \.self) { Text($0) }
                } header: {
                    Text("Fixed to make sense")
                } footer: {
                    Text("These didn't fit the conversation, so they were changed to what you most likely said.")
                }
            }
            if record.raw != record.text {
                Section("What you said") {
                    Text(record.raw).textSelection(.enabled)
                    Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = record.raw }
                }
            }
            Section {
                Button("It heard me wrong", systemImage: "ear.badge.waveform") {
                    corrected = record.raw
                    correcting = true
                }
            } footer: {
                Text("Tell it what you meant, and it learns from the difference.")
            }
        }
        .navigationTitle(record.date.formatted(date: .abbreviated, time: .shortened))
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $correcting) { correctionSheet }
        .alert(notice ?? "", isPresented: Binding(get: { notice != nil }, set: { if !$0 { notice = nil } })) {
            Button("OK") { notice = nil }
        }
    }

    /// Fix the words WhimprFlow got wrong; it learns from the difference.
    private var correctionSheet: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("What you meant", text: $corrected, axis: .vertical)
                        .lineLimit(3...10)
                } header: {
                    Text("What did you mean to say?")
                } footer: {
                    Text("Only change the words it got wrong. WhimprFlow compares this with what it heard and learns how you sound.")
                }
            }
            .navigationTitle("Correct it")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { correcting = false } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Learn") {
                        do {
                            try brain.learn(meant: corrected, heard: record.raw)
                            correcting = false
                        } catch {
                            notice = error.localizedDescription
                        }
                    }
                    .disabled(corrected.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || corrected == record.raw)
                }
            }
        }
    }
}
