#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)] // Hits predate the gate
#![allow(clippy::expect_used)] // Hits predate the gate
#![allow(clippy::string_slice)]
#![allow(clippy::unwrap_used)] // Hits predate the gate
#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
//! Shared test utilities for grok-build crates.
#![deny(clippy::indexing_slicing)]
/// CI lanes on shared runner pools raise it so pool load slows tests instead of failing them (see the Grok Build merge CI workflow).
pub fn scaled(base: std::time::Duration) -> std::time::Duration {
    let scale = std::env::var("GROK_TEST_TIMEOUT_SCALE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(1);
    base * scale
}
/// Runs a test body inside a `tokio::task::LocalSet`, which the stdio clients require: their connections are
/// not `Send`, so their tasks are spawned locally.
pub async fn run_in_local_set<F, Fut>(body: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    tokio::task::LocalSet::new().run_until(body()).await;
}
mod acp_agent_connection;
mod acp_agent_process;
mod acp_ask_user_question;
mod acp_client;
mod acp_hold_registry;
mod acp_policy;
mod acp_scripted_client;
mod acp_test_client;
mod acp_transcript;
mod bounded_log;
mod conversation;
mod conversation_replay;
mod conversation_script;
pub mod counting_server;
pub mod env;
mod failure;
mod feedback_endpoint;
mod gated_upload_proxy;
pub mod headless;
mod inference_override;
mod inference_request;
mod inference_route;
#[cfg(unix)]
pub mod leader;
mod loopback;
#[cfg(test)]
mod loopback_client;
mod mock_otel_server;
pub mod mock_server;
mod mock_server_tls;
mod model_reply;
mod otel_decode;
mod otel_event;
#[cfg(test)]
mod otel_fixtures;
mod otel_recorder;
pub mod process;
mod request_log;
pub mod resources;
pub mod sandbox;
pub mod scripted;
pub mod sse;
mod storage_endpoint;
mod telemetry_events;
mod tool_call_turn;
mod tools;
#[cfg(unix)]
pub mod uds_proxy;
mod watched;
pub use acp_client::{GrokStdioClient, SpawnOptions};
pub use acp_policy::{
    ClientPolicy, ElicitationDecision, Interactivity, PermissionDecision, QuestionDecision,
    RequestPolicy, TrustDecision,
};
pub use acp_test_client::AcpTestClient;
pub use acp_transcript::TranscriptEntry;
pub use conversation::ReadConversation;
pub use conversation_script::{Conversation, MockToolCall, ScriptViolation, mock_call_id};
pub use counting_server::spawn_counting_server;
pub use env::{
    EnvGuard, ensure_cargo_bin_with_features, git_workdir, grok_binary, isolate_grok_env,
    set_grok_binary_override,
};
pub use failure::{
    CUT_REPLY, DOOM_LOOP_CHECK_HEADER, DOOM_LOOP_TRIGGER, ErrorPosition, LOOPING_REPLY,
    ObservedFailure, StatusFailure, StreamError,
};
pub use headless::{
    HeadlessResult, assert_headless_success, assert_no_crashes, run_headless,
    run_headless_in_sandbox, run_headless_in_sandbox_borrowed,
    run_headless_in_sandbox_borrowed_with_env, run_headless_in_sandbox_with_env,
    run_headless_with_env, stderr_tail,
};
pub use inference_override::{InferenceExpectation, InferenceRequestMatcher};
pub use inference_request::{DEFAULT_MODEL, InferenceEndpoint};
#[cfg(unix)]
pub use leader::LeaderFixture;
pub use mock_otel_server::MockOtelServer;
pub use mock_server::{
    FeedbackPost, GatedUploadProxy, MockCanAdministerTeam, MockInferenceServer, MockModelEntry,
    MockUserTeam, ScriptedResponse, SseEvent, StorageUpload,
};
pub use otel_event::{
    OtelAttributes, OtelBody, OtelDecodeError, OtelEvent, OtelExport, OtelFault, OtelLogRecord,
    OtelMetricData, OtelMetricPoint, OtelNumber, OtelSignal, OtelSpan, OtelTemporality,
    OtelUnreadBody,
};
pub use otel_recorder::{OtelRecorder, OtelRecorderError};
#[cfg(unix)]
pub use process::process_has_exited_without_reap;
pub use process::{
    TestOutput, TestOutputSnapshot, TestProcess, TestProcessConfig, TestProcessState,
    TestProcessStderr, TestProcessStdout, TestProcessTermination, TestProcessTree, TestStdin,
};
pub use resources::{ResourceGrowth, ResourceSnapshot, RssMeasurement, RssOutcome, RssSampler};
pub use sandbox::{TestSandbox, TestSandboxBuilder};
pub use tools::{DAEMON_SPAWN_TOOL, GROK_BUILD_SPAWN_TOOL, Tool};
