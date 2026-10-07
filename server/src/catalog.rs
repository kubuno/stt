//! Static catalog of available STT models (Whisper, multilingual).
//!
//! Whisper is the only engine: whisper.cpp is compiled from source and linked
//! statically into the binary, so the `.kbpkg` stays self-contained and portable
//! across Linux/Windows/macOS with no external native library to install.

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct WhisperModel {
    pub id:      &'static str,   // file = <id>.bin
    pub label:   &'static str,
    pub size_mb: u32,
    pub url:     &'static str,
}

macro_rules! whisper {
    ($id:literal, $label:literal, $mb:literal) => {
        WhisperModel { id: $id, label: $label, size_mb: $mb,
                       url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/", $id, ".bin") }
    };
}

// Whisper is multilingual: one model serves every language (language is passed at
// inference time). Larger = more accurate but heavier.
pub fn whisper_models() -> Vec<WhisperModel> {
    vec![
        whisper!("ggml-tiny",     "Whisper tiny",     75),
        whisper!("ggml-base",     "Whisper base",     142),
        whisper!("ggml-small",    "Whisper small",    466),
        whisper!("ggml-medium",   "Whisper medium",   1500),
        whisper!("ggml-large-v3", "Whisper large-v3", 2900),
    ]
}

/// Languages supported by the app (codes mirror the core i18n list).
pub fn languages() -> Vec<(&'static str, &'static str)> {
    vec![
        ("en", "English"), ("fr", "Français"), ("es", "Español"), ("pt", "Português"),
        ("it", "Italiano"), ("de", "Deutsch"), ("el", "Ελληνικά"), ("ru", "Русский"),
        ("ar", "العربية"), ("he", "עברית"), ("hi", "हिन्दी"), ("zh", "中文"), ("ja", "日本語"),
    ]
}

/// Resolve the download URL for a Whisper model id. `engine` is kept for API
/// stability (only "whisper" is valid now).
pub fn model_url(engine: &str, id: &str) -> Option<String> {
    match engine {
        "whisper" => whisper_models().iter().find(|m| m.id == id).map(|m| m.url.to_string()),
        _ => None,
    }
}
