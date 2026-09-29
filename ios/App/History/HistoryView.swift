import SwiftUI

struct HistoryView: View {
    let history: DictationHistory
    @State private var confirmingClear = false

    var body: some View {
        List {
            if history.records.isEmpty {
                Text("Your dictations will appear here.")
                    .foregroundStyle(.secondary)
            }
            ForEach(history.records) { record in
                NavigationLink {
                    RecordDetail(record: record)
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

    var body: some View {
        List {
            Section("Inserted") {
                Text(record.text).textSelection(.enabled)
                Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = record.text }
            }
            if record.raw != record.text {
                Section("What you said") {
                    Text(record.raw).textSelection(.enabled)
                    Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = record.raw }
                }
            }
        }
        .navigationTitle(record.date.formatted(date: .abbreviated, time: .shortened))
        .navigationBarTitleDisplayMode(.inline)
    }
}
