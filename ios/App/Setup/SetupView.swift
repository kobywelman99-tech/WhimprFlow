import SwiftUI

/// The first-run checklist. Opens by itself until the keyboard can dictate,
/// and from the main screen any time after.
struct SetupView: View {
    let model: SetupModel
    let settings: CleanupSettings
    let cleanup: CleanupPipeline

    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @State private var showingSettings = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(SetupStep.allCases, id: \.self) { step in
                        StepRow(
                            step: step,
                            number: (SetupStep.allCases.firstIndex(of: step) ?? 0) + 1,
                            done: model.snapshot.isDone(step),
                            isNext: model.snapshot.nextStep == step,
                            microphone: model.snapshot.microphone,
                            action: { perform(step) }
                        )
                    }
                } footer: {
                    Text(model.snapshot.isReady
                         ? "You're set. Tap the mic on the WhimprFlow keyboard in any app."
                         : "Steps tick themselves off as you go.")
                }

                if let expires = model.buildExpires {
                    Section {
                        Label {
                            Text("This test build works until \(expires.formatted(date: .abbreviated, time: .omitted)). To renew it, plug the iPhone into the Mac and run ios/dev.sh phone.")
                        } icon: {
                            Image(systemName: "calendar.badge.clock")
                                .foregroundStyle(model.buildExpiresSoon() ? .orange : .secondary)
                        }
                        .font(.footnote)
                    }
                }
            }
            .navigationTitle("Set up WhimprFlow")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(model.snapshot.isReady ? "Done" : "Later") { dismiss() }
                }
            }
            .sheet(isPresented: $showingSettings, onDismiss: model.refresh) {
                SettingsView(settings: settings, pipeline: cleanup)
            }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { model.refresh() }
            }
            .onAppear(perform: model.refresh)
        }
    }

    private func perform(_ step: SetupStep) {
        switch step {
        case .microphone:
            if model.snapshot.microphone == .denied {
                model.openSettings()
            } else {
                Task { await model.requestMicrophone() }
            }
        case .keyboard, .fullAccess:
            model.openSettings()
        case .apiKey:
            showingSettings = true
        case .firstDictation:
            dismiss()
        }
    }
}

private struct StepRow: View {
    let step: SetupStep
    let number: Int
    let done: Bool
    let isNext: Bool
    let microphone: SetupSnapshot.Microphone
    let action: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            ZStack {
                Circle()
                    .fill(done ? Color.green : (isNext ? Color.accentColor : Color.secondary.opacity(0.25)))
                    .frame(width: 28, height: 28)
                if done {
                    Image(systemName: "checkmark").font(.footnote.weight(.bold)).foregroundStyle(.white)
                } else {
                    Text("\(number)").font(.footnote.weight(.semibold).monospacedDigit())
                        .foregroundStyle(isNext ? .white : .secondary)
                }
            }
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(title).font(.body.weight(.semibold))
                    if step.isOptional {
                        Text("Optional").font(.caption).foregroundStyle(.secondary)
                    }
                }
                if !done {
                    Text(detail).font(.subheadline).foregroundStyle(.secondary)
                }
                if !done, let actionTitle {
                    Button(actionTitle, action: action)
                        .buttonStyle(.borderedProminent)
                        .tint(isNext ? .accentColor : .secondary)
                        .controlSize(.small)
                        .padding(.top, 4)
                }
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityValue(done ? "Done" : "Not done")
    }

    private var title: String {
        switch step {
        case .microphone: "Allow the microphone"
        case .keyboard: "Add the WhimprFlow keyboard"
        case .fullAccess: "Allow Full Access"
        case .apiKey: "Add your Anthropic key"
        case .firstDictation: "Dictate from the keyboard"
        }
    }

    private var detail: String {
        switch step {
        case .microphone:
            microphone == .denied
                ? "Microphone access is off. Turn it on in Settings."
                : "WhimprFlow listens only while you dictate."
        case .keyboard:
            "In Settings, tap Keyboards and turn on WhimprFlow."
        case .fullAccess:
            "On the same screen, turn on Allow Full Access, then tap the keyboard's mic once in any app. This ticks when the keyboard connects."
        case .apiKey:
            "Claude cleans up fillers and punctuation. Without a key, your exact words are inserted."
        case .firstDictation:
            "Tap the mic, speak, tap stop. The first time, WhimprFlow opens to turn Flow on; tap ◀ to go back."
        }
    }

    private var actionTitle: String? {
        switch step {
        case .microphone: microphone == .denied ? "Open Settings" : "Allow"
        case .keyboard, .fullAccess: "Open Settings"
        case .apiKey: "Add key"
        case .firstDictation: "Try it"
        }
    }
}
