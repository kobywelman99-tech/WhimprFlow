import SwiftUI

@main
struct WhimprFlowApp: App {
    @State private var settings: CleanupSettings
    @State private var dictionary: DictionaryModel
    @State private var dictation: DictationService
    @State private var flow: FlowController
    @State private var setup: SetupModel
    private let cleanup: CleanupPipeline

    init() {
        let settings = CleanupSettings()
        let dictionary = DictionaryModel()
        let cleanup = CleanupPipeline(settings: settings, vocabulary: { dictionary.relevant(to: $0) })
        let dictation = DictationService(cleanup: cleanup, vocabulary: { dictionary.words })
        _settings = State(initialValue: settings)
        _dictionary = State(initialValue: dictionary)
        _dictation = State(initialValue: dictation)
        _flow = State(initialValue: FlowController(dictation: dictation, settings: settings))
        _setup = State(initialValue: SetupModel(settings: settings))
        self.cleanup = cleanup
    }

    var body: some Scene {
        WindowGroup {
            ContentView(dictation: dictation, flow: flow, settings: settings, cleanup: cleanup, dictionary: dictionary, setup: setup)
                .onOpenURL { url in
                    Task { await flow.handle(url: url) }
                }
        }
    }
}
