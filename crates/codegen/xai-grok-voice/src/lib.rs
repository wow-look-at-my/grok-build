#![allow(clippy::cast_lossless)] // Hits predate the gate
#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)] // Hits predate the gate
#![allow(clippy::cast_sign_loss)]
//! Voice input for Grok Build CLI: an xAI streaming STT client and the [`run_voice_pipeline`] task that emits [`VoiceEvent`]s for the pager.

#![deny(clippy::indexing_slicing)]

#[cfg(feature = "audio")]
pub mod audio;
pub mod auth;
pub mod config;
pub mod error;
pub mod event;
pub mod language;
pub mod pipeline;
pub mod probe;
pub mod stt;

pub use auth::{SharedVoiceAuth, StaticVoiceAuth, VoiceAuthError, VoiceAuthProvider};
pub use config::VoiceConfig;
pub use error::VoiceError;
pub use event::VoiceEvent;
pub use language::{
    STT_LANGUAGE_AUTO, STT_LANGUAGE_DEFAULT, STT_LANGUAGES, SttLanguage, canonicalize_stt_language,
    language_for_api, stt_language_by_code,
};
pub use pipeline::{VoiceCommand, run_voice_pipeline};
#[cfg(feature = "audio")]
pub use probe::run_mic_only_probe;
pub use probe::{
    InputDeviceInfo, VoiceProbeOptions, VoiceProbeReport, format_probe_report, input_device_info,
    run_streaming_probe,
};

/// Linux shells out to a system recorder (`pw-record`/`parec`/`arecord`).
pub const AUDIO_SUPPORTED: bool = cfg!(feature = "audio");

/// Hidden subcommand consumers re-exec themselves with to capture microphone audio in a short-lived helper process.
pub const MIC_CAPTURE_SUBCOMMAND: &str = "__mic-capture";

/// If this process was re-exec'd as the hidden mic-capture helper, run it and return `Some(exit_code)`; otherwise `None` (a normal invocation).
/// Call at the very top of `main` in every binary that links this crate with `audio` (the pager composition root and `voice-probe`).
/// This mirrors the pager's mermaid render child intercept.
pub fn maybe_run_capture_subprocess() -> Option<i32> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if !is_capture_subcommand(&argv) {
        return None;
    }
    #[cfg(all(feature = "audio", not(target_os = "linux")))]
    {
        let args: Vec<String> = argv
            .into_iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        Some(audio::run_capture_child_cli(args))
    }
    #[cfg(not(all(feature = "audio", not(target_os = "linux"))))]
    {
        // This build's own parent backend never spawns the helper (Linux uses system recorders; no-audio builds have no capture).
        use std::io::Write;
        let _ = writeln!(
            std::io::stdout(),
            "ERR mic-capture helper unavailable in this build"
        );
        Some(2)
    }
}

fn is_capture_subcommand(argv: &[std::ffi::OsString]) -> bool {
    argv.get(1).and_then(|a| a.to_str()) == Some(MIC_CAPTURE_SUBCOMMAND)
}

#[cfg(test)]
mod intercept_tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<std::ffi::OsString> {
        items.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn capture_subcommand_matches_only_argv1() {
        assert!(is_capture_subcommand(&argv(&["grok", "__mic-capture"])));
        assert!(is_capture_subcommand(&argv(&[
            "grok",
            "__mic-capture",
            "--rate",
            "16000"
        ])));
        assert!(!is_capture_subcommand(&argv(&["grok"])));
        assert!(!is_capture_subcommand(&argv(&["grok", "chat"])));
        assert!(!is_capture_subcommand(&argv(&[
            "grok",
            "chat",
            "__mic-capture"
        ])));
    }
}
