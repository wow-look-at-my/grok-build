//! On-disk session synthesis.

pub mod bench;
pub mod replay;

pub use bench::{make_session_with_size, make_session_with_size_blocking};
pub use replay::{
    SessionSpec, expected_replay_lines, locate_session_dir, prepare_session, sid,
    write_rewind_jsonl,
};
