//! Layer-2 stream transforms: turn raw HTTP chunk streams into [`SamplingEvent`](crate::events::SamplingEvent) streams.

pub mod chat_completions;
pub mod collect;
pub mod messages;
pub mod ollama;
pub mod responses;

pub use chat_completions::stream_chat_completions;
pub use collect::collect_response;
pub use messages::stream_messages;
pub use ollama::stream_ollama;
pub use responses::stream_responses;
