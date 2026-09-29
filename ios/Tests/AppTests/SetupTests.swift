import Foundation
import XCTest
@testable import WhimprFlow

final class SetupSnapshotTests: XCTestCase {
    func testFreshInstallStartsAtTheMicrophone() {
        let snapshot = SetupSnapshot()
        XCTAssertEqual(snapshot.nextStep, .microphone)
        XCTAssertFalse(snapshot.isReady)
        XCTAssertEqual(snapshot.doneCount, 0)
    }

    func testStepsGoInOrderAndTheKeyIsLast() {
        var snapshot = SetupSnapshot(microphone: .granted)
        XCTAssertEqual(snapshot.nextStep, .keyboard)
        snapshot.keyboardEnabled = true
        XCTAssertEqual(snapshot.nextStep, .fullAccess)
        snapshot.keyboardHasConnected = true
        XCTAssertTrue(snapshot.isReady)
        XCTAssertEqual(snapshot.nextStep, .firstDictation, "required steps before the optional key")
        snapshot.keyboardHasDictated = true
        XCTAssertEqual(snapshot.nextStep, .apiKey)
        snapshot.hasAPIKey = true
        XCTAssertNil(snapshot.nextStep)
        XCTAssertEqual(snapshot.doneCount, SetupStep.allCases.count)
    }

    func testAConnectionProvesTheKeyboardWasAdded() {
        // iOS may not list keyboards; the keyboard reaching the app settles it.
        let snapshot = SetupSnapshot(microphone: .granted, keyboardEnabled: nil, keyboardHasConnected: true)
        XCTAssertTrue(snapshot.isDone(.keyboard))
        XCTAssertTrue(snapshot.isDone(.fullAccess))
        XCTAssertTrue(snapshot.isReady)
    }

    func testDeniedMicrophoneIsNotDone() {
        XCTAssertFalse(SetupSnapshot(microphone: .denied).isDone(.microphone))
    }

    @MainActor
    func testKeyboardListMatching() {
        let id = "com.whimpr.whimprflow.ios.keyboard"
        XCTAssertEqual(SetupModel.isKeyboardEnabled(enabledKeyboards: ["en_US@sw=QWERTY", id], keyboardID: id), true)
        XCTAssertEqual(SetupModel.isKeyboardEnabled(enabledKeyboards: ["en_US@sw=QWERTY"], keyboardID: id), false)
        XCTAssertNil(SetupModel.isKeyboardEnabled(enabledKeyboards: nil, keyboardID: id), "unknown, not 'no'")
    }

    func testProgressIsRemembered() {
        let defaults = UserDefaults(suiteName: "setup-\(UUID().uuidString)")!
        let progress = SetupProgress(defaults: defaults)
        XCTAssertFalse(progress.keyboardHasConnected)
        progress.markKeyboardConnected()
        progress.markKeyboardDictated()
        let later = SetupProgress(defaults: defaults)
        XCTAssertTrue(later.keyboardHasConnected)
        XCTAssertTrue(later.keyboardHasDictated)
    }
}

final class ProvisioningInfoTests: XCTestCase {
    func testReadsExpiryFromASignedProfile() throws {
        let expiry = Date(timeIntervalSince1970: 1_790_000_000)
        let plist = try PropertyListSerialization.data(
            fromPropertyList: ["Name": "iOS Team Provisioning Profile", "ExpirationDate": expiry],
            format: .xml,
            options: 0
        )
        // Signature bytes around the list, as in a real .mobileprovision.
        let profile = Data([0x30, 0x82, 0x3C, 0x00, 0xFF]) + plist + Data([0xA0, 0x82, 0x0B, 0x00])
        XCTAssertEqual(ProvisioningInfo.expirationDate(profile: profile), expiry)
    }

    func testGarbageHasNoExpiry() {
        XCTAssertNil(ProvisioningInfo.expirationDate(profile: Data("not a profile".utf8)))
        XCTAssertNil(ProvisioningInfo.expirationDate(profile: Data("<?xml broken".utf8)))
    }

    /// On the iPhone the real profile must parse; simulator builds have none.
    func testThisBuildsProfile() throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Simulator builds aren't provisioned")
        #else
        let expiry = try XCTUnwrap(ProvisioningInfo.currentExpirationDate())
        print("BUILD expires \(expiry)")
        XCTAssertGreaterThan(expiry, Date(), "this build hasn't expired")
        print("KEYBOARDS \(UserDefaults.standard.array(forKey: "AppleKeyboards") ?? ["<none>"])")
        #endif
    }
}
