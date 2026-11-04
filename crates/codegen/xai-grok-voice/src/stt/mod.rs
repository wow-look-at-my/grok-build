//! xAI Speech-to-Text: streaming `wss://api.example.invalid/v1/stt`.

mod streaming;
mod types;

pub use streaming::{StreamingSttEvent, StreamingSttSession};
pub use types::{SttServerEvent, SttTranscriptPartial};
