use super::*;
use futures_util::stream;
use parking_lot::Mutex;

#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<BilledCall>>,
}

impl UsageSink for Recorder {
    fn record_call(&self, call: BilledCall) {
        self.calls.lock().push(call);
    }
}

fn recorder() -> (Arc<Recorder>, SharedUsageSink) {
    let recorder = Arc::new(Recorder::default());
    let sink: SharedUsageSink = recorder.clone();
    (recorder, sink)
}

fn chat_chunk(model: &str, usage: Option<(u32, u32, Option<i64>)>) -> ChatCompletionChunk {
    let mut value = serde_json::json!({
        "id": "c",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": model,
        "choices": [],
    });
    if let Some((prompt, completion, ticks)) = usage {
        value["usage"] = serde_json::json!({
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "total_tokens": prompt + completion,
            "cost_in_usd_ticks": ticks,
        });
    }
    serde_json::from_value(value).expect("chunk parses")
}

#[test]
fn a_meter_reports_once_and_names_the_reported_model() {
    let (recorder, sink) = recorder();
    let mut meter = CallMeter::new(Some(sink), "asked-model".into());
    observe_chat_chunk(&mut meter, &chat_chunk("served-model", None));
    observe_chat_chunk(
        &mut meter,
        &chat_chunk("served-model", Some((10, 2, Some(7)))),
    );
    meter.finish();
    meter.finish();
    drop(meter);

    let calls = recorder.calls.lock();
    assert_eq!(
        calls.len(),
        1,
        "finish and drop report one call between them"
    );
    let call = calls.first().expect("one call");
    assert_eq!(call.model, "served-model");
    assert_eq!(call.cost_usd_ticks, Some(7));
    assert_eq!(call.usage.as_ref().map(|u| u.prompt_tokens), Some(10));
}

#[test]
fn a_meter_falls_back_to_the_request_model() {
    let (recorder, sink) = recorder();
    let mut meter = CallMeter::new(Some(sink), "asked-model".into());
    observe_chat_chunk(&mut meter, &chat_chunk("", Some((1, 1, None))));
    drop(meter);
    assert_eq!(
        recorder.calls.lock().first().map(|c| c.model.clone()),
        Some("asked-model".into())
    );
}

#[test]
fn a_call_with_neither_usage_nor_cost_reports_nothing() {
    let (recorder, sink) = recorder();
    let mut meter = CallMeter::new(Some(sink), "m".into());
    observe_chat_chunk(&mut meter, &chat_chunk("m", None));
    drop(meter);
    assert!(recorder.calls.lock().is_empty());
}

#[tokio::test]
async fn a_metered_stream_reports_at_its_end() {
    let (recorder, sink) = recorder();
    let items: Vec<Result<ChatCompletionChunk>> = vec![
        Ok(chat_chunk("m", None)),
        Ok(chat_chunk("m", Some((5, 3, None)))),
    ];
    let mut metered = meter_stream(
        stream::iter(items).boxed(),
        CallMeter::new(Some(sink), "m".into()),
        observe_chat_chunk,
    );
    while metered.next().await.is_some() {
        assert!(recorder.calls.lock().is_empty(), "no report before the end");
    }
    assert_eq!(recorder.calls.lock().len(), 1, "the end reports the call");
    drop(metered);
    assert_eq!(
        recorder.calls.lock().len(),
        1,
        "the drop does not report again"
    );
}

#[tokio::test]
async fn a_stream_dropped_after_usage_arrived_reports_what_arrived() {
    let (recorder, sink) = recorder();
    let items: Vec<Result<ChatCompletionChunk>> = vec![
        Ok(chat_chunk("m", Some((5, 1, None)))),
        Ok(chat_chunk("m", None)),
    ];
    let mut metered = meter_stream(
        stream::iter(items).boxed(),
        CallMeter::new(Some(sink), "m".into()),
        observe_chat_chunk,
    );
    let _ = metered.next().await;
    drop(metered);
    let calls = recorder.calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls
            .first()
            .and_then(|c| c.usage.as_ref())
            .map(|u| u.prompt_tokens),
        Some(5)
    );
}

#[tokio::test]
async fn a_stream_that_fails_after_usage_arrived_reports_what_arrived() {
    let (recorder, sink) = recorder();
    let items: Vec<Result<ChatCompletionChunk>> = vec![
        Ok(chat_chunk("m", Some((4, 1, Some(9))))),
        Err(xai_grok_sampling_types::SamplingError::EventStreamError(
            "connection reset".into(),
        )),
    ];
    let metered = meter_stream(
        stream::iter(items).boxed(),
        CallMeter::new(Some(sink), "m".into()),
        observe_chat_chunk,
    );
    let _: Vec<_> = metered.collect().await;
    assert_eq!(
        recorder.calls.lock().first().and_then(|c| c.cost_usd_ticks),
        Some(9)
    );
}

#[tokio::test]
async fn a_messages_stream_reports_on_message_stop() {
    let (recorder, sink) = recorder();
    let start: messages::MessageStreamEvent = serde_json::from_value(serde_json::json!({
        "type": "message_start",
        "message": {
            "id": "msg", "type": "message", "role": "assistant", "content": [],
            "model": "claude-x", "stop_reason": null,
            "usage": { "input_tokens": 3, "output_tokens": 1, "cache_read_input_tokens": 7 }
        }
    }))
    .expect("message_start parses");
    let delta: messages::MessageStreamEvent = serde_json::from_value(serde_json::json!({
        "type": "message_delta",
        "delta": { "stop_reason": "end_turn" },
        "usage": { "output_tokens": 12 }
    }))
    .expect("message_delta parses");
    let items: Vec<Result<messages::MessageStreamEvent>> = vec![
        Ok(start),
        Ok(delta),
        Ok(messages::MessageStreamEvent::MessageStop),
        Ok(messages::MessageStreamEvent::Ping),
    ];
    let mut metered = meter_stream(
        stream::iter(items).boxed(),
        CallMeter::new(Some(sink), "asked".into()),
        observe_messages_event,
    );
    for _ in 0..3 {
        let _ = metered.next().await;
    }
    let calls = recorder.calls.lock().clone();
    assert_eq!(
        calls.len(),
        1,
        "message_stop reports before the stream ends"
    );
    let call = calls.first().expect("one call");
    assert_eq!(call.model, "claude-x");
    let usage = call.usage.as_ref().expect("usage");
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.cached_prompt_tokens, 7);
    assert_eq!(usage.completion_tokens, 12);
}

#[tokio::test]
async fn a_meter_without_a_sink_leaves_the_stream_alone() {
    let items: Vec<Result<ChatCompletionChunk>> = vec![Ok(chat_chunk("m", Some((1, 1, None))))];
    let metered = meter_stream(
        stream::iter(items).boxed(),
        CallMeter::new(None, "m".into()),
        observe_chat_chunk,
    );
    assert_eq!(metered.collect::<Vec<_>>().await.len(), 1);
}
