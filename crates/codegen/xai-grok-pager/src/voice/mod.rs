//! Voice input: STT pipeline integration and prompt-box dictation.

mod auth;
mod handle;

pub use auth::build_voice_auth;
pub use handle::handle_voice_event;
pub(crate) use handle::{
    VoiceInterimCommit, commit_interim_into_prompt, merge_voice_fragment, prompt_blank_for_voice,
    space_voice_fragment,
};
// Re-exported for the composition-root binary, which links the pager library.
pub use xai_grok_voice::maybe_run_capture_subprocess;
