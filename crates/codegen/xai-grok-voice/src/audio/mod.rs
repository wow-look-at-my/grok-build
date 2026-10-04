//! Microphone capture (optional `audio` feature).

// cpal-based capture: the Windows backend, the macOS fallback, and the macOS `__mic-capture` child implementation
#[cfg(not(target_os = "linux"))]
mod capture;
// Wire protocol shared by the `__mic-capture` child (writer, in `capture`) and the macOS parent (parser, in `capture_subprocess`)
#[cfg(not(target_os = "linux"))]
mod protocol;
#[cfg(not(target_os = "linux"))]
pub use capture::capture_pcm_for_duration;
#[cfg(not(target_os = "linux"))]
pub(crate) use capture::run_capture_child_cli;
#[cfg(target_os = "windows")]
pub use capture::{CaptureHandle, input_device_info, spawn_pcm_capture};

// Shared PCM-over-pipe handling for both subprocess backends
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod pipe;

#[cfg(target_os = "macos")]
mod capture_subprocess;
#[cfg(target_os = "macos")]
pub use capture_subprocess::{CaptureHandle, input_device_info, spawn_pcm_capture};

#[cfg(target_os = "linux")]
mod capture_linux;
#[cfg(target_os = "linux")]
pub use capture_linux::{
    CaptureHandle, capture_pcm_for_duration, input_device_info, spawn_pcm_capture,
};
