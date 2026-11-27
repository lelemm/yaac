//! The language of voice calls and dictation (docs/plan/voice-call.md § Language): what the
//! renderer sent (Android sends its own setting or the phone's language), else Settings →
//! Voice call language, else this computer's display language.

use crate::app::YaacApp;

/// This computer's display language as a BCP-47 tag, read once.
pub(crate) fn device() -> Option<String> {
    static DEVICE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    DEVICE.get_or_init(read_device).clone()
}

#[cfg(windows)]
fn read_device() -> Option<String> {
    yaac_winsys::user_ui_language().and_then(|n| yaac_core::voice_language::normalize(&n))
}

#[cfg(not(windows))]
fn read_device() -> Option<String> {
    // The POSIX order: LC_ALL overrides LC_MESSAGES, which overrides LANG.
    ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().filter_map(|k| std::env::var(k).ok()).find(|v| !v.is_empty())
        .and_then(|v| yaac_core::voice_language::from_posix(&v))
}

/// The language to send with a call or a dictation started from `renderer`'s command.
pub(crate) fn language(app: &YaacApp, renderer: Option<&str>) -> Option<String> {
    renderer.and_then(yaac_core::voice_language::normalize)
        .or_else(|| yaac_core::voice_language::effective(&app.ui.voice_language, device().as_deref()))
}
