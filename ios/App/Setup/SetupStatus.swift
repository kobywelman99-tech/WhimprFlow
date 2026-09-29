import AVFAudio
import Foundation
import Observation
import UIKit

/// The first-run checklist. Each step ticks itself from what the app can
/// observe rather than asking the user to confirm.
enum SetupStep: CaseIterable, Equatable {
    case microphone
    /// Added in Settings ▸ Keyboards.
    case keyboard
    /// Allow Full Access. iOS doesn't tell the app, but the keyboard can only
    /// reach the app with it on, so the first connection proves it.
    case fullAccess
    /// Optional: without a key, dictation works but isn't cleaned up.
    case apiKey
    case firstDictation

    var isOptional: Bool { self == .apiKey }
}

struct SetupSnapshot: Equatable {
    enum Microphone: Equatable { case undetermined, granted, denied }

    var microphone: Microphone = .undetermined
    /// nil when iOS doesn't say which keyboards are enabled.
    var keyboardEnabled: Bool?
    var keyboardHasConnected = false
    var hasAPIKey = false
    var keyboardHasDictated = false

    func isDone(_ step: SetupStep) -> Bool {
        switch step {
        case .microphone: microphone == .granted
        case .keyboard: keyboardEnabled == true || keyboardHasConnected
        case .fullAccess: keyboardHasConnected
        case .apiKey: hasAPIKey
        case .firstDictation: keyboardHasDictated
        }
    }

    /// The first step still to do, required ones before optional ones.
    var nextStep: SetupStep? {
        SetupStep.allCases.first { !$0.isOptional && !isDone($0) }
            ?? SetupStep.allCases.first { !isDone($0) }
    }

    /// Everything needed to dictate from the keyboard.
    var isReady: Bool {
        [.microphone, .keyboard, .fullAccess].allSatisfy(isDone)
    }

    var doneCount: Int { SetupStep.allCases.filter(isDone).count }
}

/// What the app has seen the keyboard do, kept across launches.
struct SetupProgress {
    var defaults: UserDefaults = .standard

    var keyboardHasConnected: Bool { defaults.bool(forKey: "setup.keyboardConnected") }
    var keyboardHasDictated: Bool { defaults.bool(forKey: "setup.keyboardDictated") }

    func markKeyboardConnected() { defaults.set(true, forKey: "setup.keyboardConnected") }
    func markKeyboardDictated() { defaults.set(true, forKey: "setup.keyboardDictated") }
}

@MainActor
@Observable
final class SetupModel {
    private(set) var snapshot = SetupSnapshot()
    /// When this free-account build stops opening (nil for simulator builds).
    let buildExpires: Date? = ProvisioningInfo.currentExpirationDate()

    @ObservationIgnored private let settings: CleanupSettings
    @ObservationIgnored private let progress: SetupProgress

    init(settings: CleanupSettings, progress: SetupProgress = SetupProgress()) {
        self.settings = settings
        self.progress = progress
        refresh()
    }

    /// Re-reads everything; call when the app comes back from Settings.
    func refresh() {
        snapshot = SetupSnapshot(
            microphone: Self.microphoneStatus(),
            keyboardEnabled: Self.isKeyboardEnabled(
                enabledKeyboards: UserDefaults.standard.array(forKey: "AppleKeyboards") as? [String],
                keyboardID: Self.keyboardBundleID
            ),
            keyboardHasConnected: progress.keyboardHasConnected,
            hasAPIKey: settings.hasAPIKey,
            keyboardHasDictated: progress.keyboardHasDictated
        )
    }

    func requestMicrophone() async {
        _ = await AVAudioApplication.requestRecordPermission()
        refresh()
    }

    /// WhimprFlow's page in Settings, which lists its Keyboards once the
    /// keyboard is installed.
    func openSettings() {
        if let url = URL(string: UIApplication.openSettingsURLString) {
            UIApplication.shared.open(url)
        }
    }

    /// True within two days of the build expiring.
    func buildExpiresSoon(now: Date = Date()) -> Bool {
        guard let buildExpires else { return false }
        return buildExpires.timeIntervalSince(now) < 2 * 24 * 60 * 60
    }

    // MARK: Probes

    static func microphoneStatus() -> SetupSnapshot.Microphone {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: .granted
        case .denied: .denied
        default: .undetermined
        }
    }

    /// The embedded keyboard's bundle ID, read from the extension itself.
    static var keyboardBundleID: String? {
        guard let plugins = Bundle.main.builtInPlugInsURL else { return nil }
        return Bundle(url: plugins.appending(path: "WhimprKeyboard.appex"))?.bundleIdentifier
    }

    /// iOS keeps the enabled keyboards in the global `AppleKeyboards`
    /// default. It isn't a documented API, so an absent list means "unknown",
    /// not "not added"; the keyboard connecting settles it either way.
    static func isKeyboardEnabled(enabledKeyboards: [String]?, keyboardID: String?) -> Bool? {
        guard let enabledKeyboards, let keyboardID else { return nil }
        return enabledKeyboards.contains(keyboardID)
    }
}

/// Reads the app's signing profile. Free Apple ID builds expire after 7 days
/// and then won't open until they're reinstalled from the Mac.
enum ProvisioningInfo {
    static func currentExpirationDate() -> Date? {
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let data = try? Data(contentsOf: url) else { return nil }
        return expirationDate(profile: data)
    }

    /// The profile is a signed envelope around a property list; pull the list
    /// out and read `ExpirationDate`.
    static func expirationDate(profile: Data) -> Date? {
        guard let start = profile.range(of: Data("<?xml".utf8)),
              let end = profile.range(of: Data("</plist>".utf8), in: start.lowerBound..<profile.endIndex),
              let plist = try? PropertyListSerialization.propertyList(from: profile[start.lowerBound..<end.upperBound], format: nil),
              let dictionary = plist as? [String: Any] else { return nil }
        return dictionary["ExpirationDate"] as? Date
    }
}
