//! Downloadable speech models: the registry and the downloader.

pub mod download;
pub mod registry;

pub use download::{download, DownloadError, Options, Progress};
pub use registry::{best_whisper_model, builtin, parakeet_dir, ModelFile, ModelSpec, Role, DEFAULT_ID, PARAKEET_ID};
