//! The scroll matrix drives the pager binary in a PTY with `GROK_SCROLL_LOG` pointed at a tempfile.

pub mod cells;
pub mod gestures;
pub mod invariants;
pub mod log;
pub mod report;
pub mod runner;
pub mod session;

pub use cells::{CELLS, ExpectedProfile, MatrixCell, Tier, curated};
pub use gestures::{GestureId, WheelStep, direction_counts};
pub use invariants::{InvariantId, InvariantResult, check_log_invariant};
pub use log::{
    EVT_FINALIZE, EVT_FLUSH, EVT_STREAM_START, ScrollLogLine, StreamGroup, group_streams,
    parse_jsonl, parse_jsonl_str, wait_for_finalize_count,
};
pub use report::{
    CellReport, CellStatus, InvariantReport, InvariantStatus, exit_code, summary_table,
    write_report_json,
};
pub use runner::run_cell;
pub use session::{
    SessionKind, marker_line, marker_response, marker_screen_row, spawn_marker_session,
    spawn_settled_marker_session, spawn_streaming_marker_session, topmost_visible_marker,
};
