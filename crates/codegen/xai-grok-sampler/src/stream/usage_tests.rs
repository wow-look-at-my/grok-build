use super::*;
use xai_grok_sampling_types::messages::{MessageDeltaUsage, MessagesUsage};

fn chat_usage(prompt: u32, completion: u32, ticks: Option<i64>, cost: Option<f64>) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        prompt_tokens_details: None,
        completion_tokens_details: None,
        cost_in_usd_ticks: ticks,
        cost: cost.map(UsageCost::from),
    }
}

#[test]
fn chat_completions_last_usage_wins_and_a_silent_cost_keeps_the_earlier_one() {
    let mut call = CallUsage::default();
    call.observe_chat_completions(&chat_usage(10, 1, Some(99), None));
    call.observe_chat_completions(&chat_usage(10, 7, Some(0), None));
    let usage = call.usage.expect("usage");
    assert_eq!((usage.prompt_tokens, usage.completion_tokens), (10, 7));
    assert_eq!(
        call.cost_usd_ticks,
        Some(99),
        "a zero is unbilled, not free"
    );
}

#[test]
fn chat_completions_converts_a_usd_float_when_no_ticks_arrive() {
    let mut call = CallUsage::default();
    call.observe_chat_completions(&chat_usage(10, 2, None, Some(0.0000416)));
    assert_eq!(call.cost_usd_ticks, Some(416_000));
}

#[test]
fn responses_reads_usage_and_takes_the_stashed_cost() {
    let sse = r#"{
        "type": "response.completed",
        "sequence_number": 0,
        "response": {
            "id": "resp_1", "object": "response", "created_at": 0,
            "model": "grok-build", "status": "completed", "output": [],
            "usage": {
                "input_tokens": 100,
                "input_tokens_details": { "cached_tokens": 40 },
                "output_tokens": 20,
                "output_tokens_details": { "reasoning_tokens": 5 },
                "total_tokens": 120,
                "cost_in_usd_ticks": 78
            }
        }
    }"#;
    let event = crate::client::deserialize_response_event(sse).expect("parse");
    let rs::ResponseStreamEvent::ResponseCompleted(mut completed) = event else {
        panic!("expected ResponseCompleted");
    };
    let call = CallUsage::from_responses(&mut completed.response);
    assert_eq!(
        call.usage,
        Some(TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
            reasoning_tokens: 5,
            cached_prompt_tokens: 40,
            cache_creation_prompt_tokens: 0,
        })
    );
    assert_eq!(call.cost_usd_ticks, Some(78));
    assert!(
        completed
            .response
            .metadata
            .as_ref()
            .is_none_or(|m| !m.contains_key(crate::client::COST_USD_TICKS_METADATA_KEY)),
        "the cost key must not reach the conversation items"
    );
}

#[test]
fn messages_folds_start_and_delta_and_keeps_a_reported_cost() {
    let mut acc = MessagesUsageAccumulator::default();
    acc.observe_start(&MessagesUsage {
        input_tokens: 10,
        output_tokens: 0,
        cache_creation_input_tokens: 3,
        cache_read_input_tokens: 50,
        cost_in_usd_ticks: Some(1_000),
        cost: None,
    });
    acc.observe_delta(&MessageDeltaUsage {
        output_tokens: 25,
        input_tokens: None,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
        cost_in_usd_ticks: None,
        cost: None,
    });
    let call = acc.finish();
    assert_eq!(
        call.usage,
        Some(TokenUsage {
            prompt_tokens: 63,
            completion_tokens: 25,
            total_tokens: 88,
            reasoning_tokens: 0,
            cached_prompt_tokens: 50,
            cache_creation_prompt_tokens: 3,
        })
    );
    assert_eq!(call.cost_usd_ticks, Some(1_000));
}

#[test]
fn messages_with_no_counts_reports_no_usage() {
    assert_eq!(
        MessagesUsageAccumulator::default().finish(),
        CallUsage::default()
    );
}

#[test]
fn ollama_reads_the_done_line_counts() {
    let chunk: OllamaChatChunk = serde_json::from_value(serde_json::json!({
        "done": true,
        "prompt_eval_count": 30,
        "prompt_eval_cached_count": 12,
        "eval_count": 8
    }))
    .expect("chunk");
    let call = CallUsage::from_ollama_done(&chunk);
    assert_eq!(
        call.usage,
        Some(TokenUsage {
            prompt_tokens: 30,
            completion_tokens: 8,
            total_tokens: 38,
            reasoning_tokens: 0,
            cached_prompt_tokens: 12,
            cache_creation_prompt_tokens: 0,
        })
    );
    assert_eq!(call.cost_usd_ticks, None);
}
