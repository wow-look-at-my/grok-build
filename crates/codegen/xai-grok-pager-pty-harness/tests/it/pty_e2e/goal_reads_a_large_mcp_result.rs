//! PTY: under an active goal, the model reads the file that holds a large MCP result.
#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::{InferenceEndpoint, InferenceRequestMatcher};
use xai_grok_test_support::{Conversation, MockToolCall, Tool, mock_call_id};

const SERVER: &str = "largeresult";
const TAIL_MARKER: &str = "LARGE_MCP_RESULT_TAIL";
const DONE: &str = "READ_THE_DUMP_DONE";
const DUMP_PATH: &str = "DUMP_PATH_FROM_THE_MCP_RESULT";

fn server_binary() -> &'static str {
    env!("CARGO_BIN_EXE_large_result_mcp_server")
}

fn seed_large_result_server(content: &ContentController) {
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create fake GROK_HOME");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[mcp_servers.{SERVER}]\ncommand = \"{}\"\nargs = []\n",
            server_binary()
        ),
    )
    .expect("write config.toml");
}

fn expect_complete_verdict(content: &ContentController) {
    let text = json!({
        "decision": "candidate_complete",
        "evidence": "the dump was read",
        "next_step": "none",
        "blocker_key": ""
    })
    .to_string();
    for (endpoint, response) in [
        (
            InferenceEndpoint::Responses,
            ScriptedResponse::sse(sse::responses_api_script_exact(&text, "test-model")),
        ),
        (
            InferenceEndpoint::ChatCompletions,
            ScriptedResponse::sse(sse::chat_completion_script_exact(&text, "test-model")),
        ),
    ] {
        let _ = content.expect_response(
            format!("complete ({endpoint:?})"),
            InferenceRequestMatcher::auxiliary(endpoint),
            response,
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn goal_reads_a_large_mcp_result_dump() {
    let content = ContentController::start().await.expect("start content");
    seed_large_result_server(&content);
    let mcp_call = MockToolCall::new(Tool::mcp(SERVER, "dump"), json!({}))
        .result_contains("Full output written to:");
    let read_call = MockToolCall::new(Tool::Read, json!({ "target_file": DUMP_PATH }))
        .argument_from_result(
            DUMP_PATH,
            mock_call_id(1, 1),
            r"Full output written to: (\S+)\.",
        )
        .expect("valid pattern")
        .result_contains(TAIL_MARKER);
    content.server().set_keep_requests(true);
    content.server().set_conversations([Conversation::nth(1)
        .calls([mcp_call, read_call])
        .reply(DONE)]);
    expect_complete_verdict(&content);
    content.set_response("GOAL_FALLBACK");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        160,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        &[
            ("GROK_GOAL", "1"),
            ("GROK_GOAL_PLANNER", "0"),
            ("GROK_GOAL_CLASSIFIER", "0"),
        ],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    // The MCP server must be connected before the round, so its tool is offered.
    harness.update(Duration::from_secs(5));
    harness
        .inject_keys(b"/goal read the large mcp result")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    let done = wait_until(Duration::from_secs(90), || {
        harness.update(Duration::from_millis(100));
        harness.screen_contents().contains(DONE)
    });
    let screen = harness.screen_contents();
    let bodies: Vec<String> = content
        .server()
        .request_bodies()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(done, "the round must finish; screen:\n{screen}");
    assert!(
        !bodies
            .iter()
            .any(|body| body.contains("Refused: this call touches")),
        "the read of the MCP dump was refused"
    );
    content.server().assert_no_script_violations();

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}
