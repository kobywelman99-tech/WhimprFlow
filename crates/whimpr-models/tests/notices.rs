//! The in-app acknowledgements must keep covering everything the app ships or downloads.

use std::path::Path;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(root().join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

#[test]
fn the_notices_name_every_component_the_app_ships_or_downloads() {
    let notices = read("ui/src/hub/third-party-notices.md");
    for needle in [
        "Parakeet",
        "CC BY 4.0",
        "istupakov",
        "OpenAI Whisper",
        "whisper.cpp",
        "Silero",
        "ONNX Runtime",
        "CMU Pronouncing Dictionary",
        "Linguistic Data",
    ] {
        assert!(notices.contains(needle), "third-party-notices.md no longer mentions {needle}");
    }
}

#[test]
fn every_downloadable_model_is_covered_by_a_notice() {
    let notices = read("ui/src/hub/third-party-notices.md");
    for model in whimpr_models::builtin() {
        let covered = if model.id == whimpr_models::PARAKEET_ID { "Parakeet" } else { "Whisper" };
        assert!(notices.contains(covered), "{} has no notice (looked for {covered})", model.id);
    }
}

#[test]
fn the_licence_texts_in_the_notices_are_the_ones_shipped_with_the_data() {
    let notices = read("ui/src/hub/third-party-notices.md");
    for file in ["crates/whimpr-core/data/CMUDICT-LICENSE", "crates/whimpr-vad/models/LICENSE-SILERO"] {
        let text = read(file);
        assert!(notices.contains(text.trim()), "the notice for {file} has drifted from the licence file");
    }
}
