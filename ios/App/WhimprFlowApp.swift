import SwiftUI

@main
struct WhimprFlowApp: App {
    @State private var settings: CleanupSettings
    @State private var dictionary: DictionaryModel
    @State private var brain: BrainModel
    @State private var history: DictationHistory
    @State private var style: StyleModel
    @State private var dictation: DictationService
    @State private var flow: FlowController
    @State private var setup: SetupModel
    private let cleanup: CleanupPipeline

    init() {
        let settings = CleanupSettings()
        let dictionary = DictionaryModel()
        let style = StyleModel()
        let brain = BrainModel()
        let cleanup = CleanupPipeline(
            settings: settings,
            vocabulary: { dictionary.relevant(to: $0) },
            replacements: { brain.correct(dictionary.applyReplacements(to: $0)) },
            styleSummary: { style.promptSummary },
            makeReviewModel: { AnthropicClient(apiKey: $0, timeout: 5) }
        )
        let history = DictationHistory()
        let dictation = DictationService(
            cleanup: cleanup,
            history: history,
            learn: { style.observe($0) },
            vocabulary: { dictionary.words + brain.biasWords },
            rescore: { brain.rescore($0) }
        )
        _settings = State(initialValue: settings)
        _dictionary = State(initialValue: dictionary)
        _brain = State(initialValue: brain)
        _history = State(initialValue: history)
        _style = State(initialValue: style)
        _dictation = State(initialValue: dictation)
        _flow = State(initialValue: FlowController(dictation: dictation, settings: settings))
        _setup = State(initialValue: SetupModel(settings: settings))
        self.cleanup = cleanup
    }

    var body: some Scene {
        WindowGroup {
            ContentView(dictation: dictation, flow: flow, settings: settings, cleanup: cleanup, dictionary: dictionary, brain: brain, history: history, style: style, setup: setup)
                .onOpenURL { url in
                    Task { await flow.handle(url: url) }
                }
        }
    }
}
