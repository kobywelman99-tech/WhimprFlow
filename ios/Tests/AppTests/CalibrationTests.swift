import Foundation
import XCTest
@testable import WhimprFlow

/// Plays back scripted recogniser output, one answer per line read.
@MainActor
private final class FakeListener: CalibrationListening {
    var answers: [String]
    private(set) var closed = false

    init(_ answers: [String]) { self.answers = answers }

    func startListening() async throws {}
    func stopListening() async throws -> String { answers.removeFirst() }
    func cancelListening() async {}
    func close() { closed = true }
}

@MainActor
final class CalibrationTests: XCTestCase {
    private func brain() -> BrainModel {
        BrainModel(fileURL: FileManager.default.temporaryDirectory
            .appending(path: "brain-\(UUID().uuidString)/brain.json"))
    }

    /// Reads every line (after `own` is added to the end) with `heard(line)` as what the recogniser returns.
    private func session(_ brain: BrainModel, own: [String] = [], heard: (String) -> String) async -> CalibrationModel {
        let listener = FakeListener([])
        let model = CalibrationModel(recorder: listener, brain: brain, lineCount: 4)
        model.begin(seed: 1)
        own.forEach(model.addOwn)
        for line in model.lines {
            listener.answers = [heard(line.text)]
            await model.startListening()
            await model.stopListening()
            model.next()
        }
        return model
    }

    func testPromptsAreServedAndStartTheReading() {
        let model = CalibrationModel(recorder: FakeListener([]), brain: brain(), lineCount: 5)
        model.begin(seed: 3)
        XCTAssertEqual(model.step, .reading)
        XCTAssertEqual(model.lines.count, 5)
        XCTAssertEqual(Set(model.lines.map(\.tag)).count, 5, "a mix of styles")
    }

    func testFinishingTeachesTheBrainFromEveryLineRead() async {
        let brain = brain()
        let model = await session(brain) { $0 }
        XCTAssertEqual(model.step, .learned(4))
        XCTAssertEqual(brain.overview.observations, 4)
        XCTAssertTrue(brain.overview.patterns.isEmpty, "a perfect reading shows no habits")
    }

    func testSkippedLinesAreNotLearnedFrom() async {
        let brain = brain()
        let listener = FakeListener(["hello there"])
        let model = CalibrationModel(recorder: listener, brain: brain, lineCount: 3)
        model.begin(seed: 1)
        await model.startListening()
        await model.stopListening()
        model.next()
        model.skip()
        model.skip()
        XCTAssertEqual(model.step, .learned(1))
        XCTAssertEqual(brain.overview.observations, 1)
    }

    func testARepeatedMishearIsFixedInLaterDictation() async {
        let brain = brain()
        _ = await session(brain, own: ["water", "water bottle"]) {
            $0.replacingOccurrences(of: "water", with: "wooder")
        }
        XCTAssertEqual(brain.overview.learnedWords, 1)
        XCTAssertEqual(brain.correct("pass the wooder"), "pass the water")
        XCTAssertEqual(brain.biasWords, ["water"], "the recogniser is nudged toward words it has botched")
    }

    func testOneOffSlipIsNotApplied() async {
        let brain = brain()
        _ = await session(brain, own: ["water"]) { $0.replacingOccurrences(of: "water", with: "wooder") }
        XCTAssertEqual(brain.correct("wooder"), "wooder")
    }

    func testEndingReadingReleasesTheMic() async {
        let listener = FakeListener(["hello"])
        let model = CalibrationModel(recorder: listener, brain: brain(), lineCount: 3)
        model.begin(seed: 1)
        await model.startListening()
        await model.stopListening()
        model.finishReading()
        XCTAssertTrue(listener.closed)
        XCTAssertEqual(model.step, .learned(1))
    }

    func testPipelineAppliesLearnedFixesEvenWhenCleanupIsOff() async throws {
        let settings = CleanupSettings(
            defaults: UserDefaults(suiteName: "cal-\(UUID().uuidString)")!,
            keychain: KeychainStore(service: "cal-tests.\(UUID().uuidString)")
        )
        settings.enabled = false
        let brain = brain()
        try brain.learn(meant: "cold water", heard: "cold wooder")
        try brain.learn(meant: "water please", heard: "wooder please")
        let pipeline = CleanupPipeline(settings: settings, replacements: { brain.correct($0) })

        let result = await pipeline.clean("a glass of wooder")
        XCTAssertEqual(result.text, "a glass of water")
    }
}

@MainActor
final class BrainModelTests: XCTestCase {
    private func brain() -> BrainModel {
        BrainModel(fileURL: FileManager.default.temporaryDirectory
            .appending(path: "brain-\(UUID().uuidString)/brain.json"))
    }

    func testAFreshBrainKeepsTheRecognisersFirstChoice() {
        XCTAssertEqual(brain().rescore(["he fought it", "he thought it"]), "he fought it")
        XCTAssertEqual(brain().rescore(["only one"]), "only one")
        XCTAssertEqual(brain().rescore([]), "")
    }

    func testLearningThForFMakesTheBrainPickTheRightAlternative() throws {
        let brain = brain()
        for _ in 0..<4 {
            try brain.learn(meant: "three", heard: "free")
            try brain.learn(meant: "thin", heard: "fin")
            try brain.learn(meant: "thread", heard: "fred")
            try brain.learn(meant: "think about it", heard: "think about it")
            try brain.learn(meant: "thank you for the thing", heard: "thank you for the thing")
        }
        // Never practised, but the same sound pattern.
        XCTAssertEqual(brain.rescore(["he fought it", "he thought it"]), "he thought it")
        XCTAssertEqual(brain.rescore(["a cat", "a dog"]), "a cat", "unrelated sounds are left alone")
        let top = try XCTUnwrap(brain.overview.patterns.first)
        XCTAssertTrue(top.meant.hasPrefix("th"), top.meant)
        XCTAssertTrue(top.heard.hasPrefix("f"), top.heard)
    }

    func testWhatItLearnedSurvivesRelaunchAndCanBeForgotten() throws {
        let url = FileManager.default.temporaryDirectory.appending(path: "brain-\(UUID().uuidString)/brain.json")
        let first = BrainModel(fileURL: url)
        try first.learn(meant: "cold water", heard: "cold wooder")
        XCTAssertEqual(BrainModel(fileURL: url).overview.observations, 1)

        try first.reset()
        XCTAssertEqual(first.overview.observations, 0)
        XCTAssertEqual(BrainModel(fileURL: url).overview.observations, 0)
    }
}
