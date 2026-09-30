//! Tests for the Ollama NDJSON transform, driven through the real stream.

use super::*;
use futures_util::stream;
use xai_grok_sampling_types::ollama::{
    OllamaChatChunk, OllamaMessage, OllamaToolCall, OllamaToolCallFunction,
};

fn chunk(json: serde_json::Value) -> Result<OllamaChatChunk, SamplingError> {
    Ok(serde_json::from_value(json).expect("chunk fixture"))
}

async fn run(chunks: Vec<Result<OllamaChatChunk, SamplingError>>) -> Vec<SamplingEvent> {
    let raw = stream::iter(chunks).boxed();
    stream_ollama(
        raw,
        None,
        RequestId::from("req-1".to_owned()),
        Duration::from_secs(5),
    )
    .collect::<Vec<_>>()
    .await
}

fn completed(events: &[SamplingEvent]) -> &ConversationResponse {
    events
        .iter()
        .find_map(|e| match e {
            SamplingEvent::Completed { response, .. } => Some(response.as_ref()),
            _ => None,
        })
        .expect("the stream completed")
}

#[tokio::test]
async fn text_chunks_accumulate_into_one_assistant_item() {
    let events = run(vec![
        chunk(serde_json::json!({
            "model": "qwen3-coder:30b",
            "message": {"role": "assistant", "content": "The "},
            "done": false
        })),
        chunk(serde_json::json!({
            "model": "qwen3-coder:30b",
            "message": {"role": "assistant", "content": "sky"},
            "done": false
        })),
        chunk(serde_json::json!({
            "model": "qwen3-coder:30b",
            "message": {"role": "assistant", "content": ""},
            "done": true, "done_reason": "stop",
            "prompt_eval_count": 26, "eval_count": 2
        })),
    ])
    .await;

    let response = completed(&events);
    let assistant = response.assistant().expect("an assistant item");
    assert_eq!(&*assistant.content, "The sky");
    assert_eq!(response.stop_reason, Some(StopReason::Stop));
    let usage = response.usage.as_ref().expect("usage from the final line");
    assert_eq!(usage.prompt_tokens, 26);
    assert_eq!(usage.completion_tokens, 2);
}

#[tokio::test]
async fn a_tool_call_arrives_whole_and_its_arguments_become_a_string() {
    let events = run(vec![
        chunk(serde_json::json!({
            "model": "m",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {"name": "bash", "arguments": {"cmd": "ls -la"}}
                }]
            },
            "done": false
        })),
        chunk(serde_json::json!({"model": "m", "done": true, "done_reason": "stop"})),
    ])
    .await;

    let delta = events
        .iter()
        .find_map(|e| match e {
            SamplingEvent::ToolCallDelta {
                arguments_delta,
                name,
                ..
            } => Some((name.clone(), arguments_delta.clone())),
            _ => None,
        })
        .expect("a tool-call delta");
    assert_eq!(delta.0.as_deref(), Some("bash"));
    assert_eq!(
        delta.1.as_deref(),
        Some(r#"{"cmd":"ls -la"}"#),
        "the whole arguments ride on one delta; a consumer waiting for a second fragment waits forever"
    );

    let response = completed(&events);
    assert_eq!(response.stop_reason, Some(StopReason::ToolCalls));
    let assistant = response.assistant().expect("an assistant item");
    assert_eq!(assistant.tool_calls.len(), 1);
    assert!(
        !assistant.tool_calls[0].id.is_empty(),
        "a call Ollama gave no id still needs one to pair with its result"
    );
}

#[tokio::test]
async fn two_calls_to_one_tool_get_distinct_ids() {
    let call = |cmd: &str| OllamaToolCall {
        id: None,
        function: OllamaToolCallFunction {
            name: "bash".to_owned(),
            arguments: serde_json::json!({ "cmd": cmd }),
        },
    };
    let message = OllamaMessage {
        role: "assistant".to_owned(),
        tool_calls: vec![call("ls"), call("pwd")],
        ..Default::default()
    };
    let events = run(vec![
        Ok(OllamaChatChunk {
            model: Some("m".to_owned()),
            message: Some(message),
            ..Default::default()
        }),
        chunk(serde_json::json!({"model": "m", "done": true, "done_reason": "stop"})),
    ])
    .await;

    let response = completed(&events);
    let assistant = response.assistant().expect("an assistant item");
    assert_eq!(assistant.tool_calls.len(), 2);
    assert_ne!(
        assistant.tool_calls[0].id, assistant.tool_calls[1].id,
        "two calls to the same tool in one message must not share an id"
    );
}

#[tokio::test]
async fn thinking_becomes_a_reasoning_item_beside_the_assistant() {
    let events = run(vec![
        chunk(serde_json::json!({
            "model": "m",
            "message": {"role": "assistant", "content": "", "thinking": "let me look"},
            "done": false
        })),
        chunk(serde_json::json!({
            "model": "m",
            "message": {"role": "assistant", "content": "done"},
            "done": false
        })),
        chunk(serde_json::json!({"model": "m", "done": true, "done_reason": "stop"})),
    ])
    .await;

    assert!(
        events.iter().any(|e| matches!(
            e,
            SamplingEvent::ChannelToken {
                channel: SamplingChannel::Reasoning,
                ..
            }
        )),
        "thinking streams on the reasoning channel as it arrives"
    );

    let response = completed(&events);
    let reasoning = response
        .items
        .iter()
        .find_map(|i| match i {
            ConversationItem::Reasoning(r) => Some(r),
            _ => None,
        })
        .expect("a reasoning item");
    assert!(
        reasoning.encrypted_content.is_none(),
        "ollama's thinking is unsigned text; a blob here would bind it to a model that never signed it"
    );
}

#[tokio::test]
async fn an_error_line_fails_the_stream_rather_than_ending_it() {
    let events = run(vec![
        chunk(serde_json::json!({
            "model": "m",
            "message": {"role": "assistant", "content": "partial"},
            "done": false
        })),
        chunk(serde_json::json!({"error": "model requires more system memory"})),
    ])
    .await;

    let failure = events.iter().find_map(|e| match e {
        SamplingEvent::Failed { error, .. } => Some(error),
        _ => None,
    });
    assert!(
        failure.is_some(),
        "a truncated answer must not be reported as a finished one"
    );
    assert!(
        failure.unwrap().message.contains("more system memory"),
        "the server's own words reach the caller: {:?}",
        failure.unwrap().message
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::Completed { .. })),
        "a failed stream never also completes"
    );
}

#[tokio::test]
async fn hitting_the_output_budget_is_a_truncation_not_a_clean_stop() {
    let events = run(vec![
        chunk(serde_json::json!({
            "model": "m",
            "message": {"role": "assistant", "content": "cut off here"},
            "done": false
        })),
        chunk(serde_json::json!({"model": "m", "done": true, "done_reason": "length"})),
    ])
    .await;

    match events.last() {
        Some(SamplingEvent::Completed { response, .. }) => {
            assert_eq!(response.stop_reason, Some(StopReason::Length));
            assert_eq!(response.assistant_text(), "cut off here");
        }
        other => panic!("expected Completed(Length), got {other:?}; events: {events:#?}"),
    }
}

#[tokio::test]
async fn the_reused_prefix_is_reported_as_a_cache_read() {
    let events = run(vec![chunk(serde_json::json!({
        "model": "m",
        "message": {"role": "assistant", "content": "hi"},
        "done": true, "done_reason": "stop",
        "prompt_eval_count": 1000, "prompt_eval_cached_count": 900, "eval_count": 5
    }))])
    .await;

    let usage = completed(&events).usage.as_ref().expect("usage").clone();
    assert_eq!(usage.prompt_tokens, 1000);
    assert_eq!(usage.cached_prompt_tokens, 900);
}
