//! A history the model refuses is converted to text, never a dead end.

use super::support::*;
use super::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use xai_grok_sampling_types::conversation::{
    AssistantItem, ConversationItem, ToolCall, ToolResultItem,
};

/// The 400 a provider answers with when the history carries an encrypted
/// reasoning blob it cannot decrypt.
fn encrypted_content_error() -> xai_grok_sampler::SamplingErrorInfo {
    xai_grok_sampler::SamplingErrorInfo {
        kind: xai_grok_sampler::SamplingErrorKind::Api,
        status_code: Some(400),
        message: "Could not decrypt the provided encrypted_content.".into(),
        is_retryable: false,
        retry_after_secs: None,
        should_retry: None,
        error_code: None,
        model_metadata: None,
        empty_response_context: None,
        doom_loop_triggers: None,
        doom_loop_aborted_at_chunk: None,
        output_rate: None,
        credential: xai_grok_sampling_types::SentCredential::Unknown,
    }
}

/// A turn that called a tool and thought about it first — the shape a model
/// switch makes unusable.
fn history_with_provider_state() -> Vec<ConversationItem> {
    let mut reasoning =
        xai_grok_sampling_types::conversation::synthesized_reasoning_item("weighing the options");
    reasoning.encrypted_content = Some("enc_blob".to_owned());
    vec![
        ConversationItem::user("read the file"),
        ConversationItem::Reasoning(reasoning),
        ConversationItem::Assistant(AssistantItem {
            content: "on it".into(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"/a"}"#.into(),
                vendor: Default::default(),
            }],
            model_id: Some("grok-4-fast".into()),
            model_fingerprint: None,
            reasoning_effort: None,
        }),
        ConversationItem::ToolResult(ToolResultItem {
            tool_call_id: "call_1".to_owned(),
            content: "fn main() {}".into(),
            images: Vec::new(),
        }),
    ]
}

/// A model that rejects the history gets the same conversation as text, and
/// the turn is resubmitted rather than ended.
#[tokio::test(flavor = "current_thread")]
async fn an_encrypted_content_rejection_flattens_and_resubmits() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _) = mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor =
                Arc::new(create_test_actor(1_000, 1_000_000, 85, gateway_tx, persistence_tx).await);
            actor
                .chat_state_handle
                .replace_conversation(history_with_provider_state());

            let result = actor
                .handle_sampling_failure(
                    encrypted_content_error(),
                    0,
                    transient_state(0, true),
                    false,
                    TurnParkState::Fresh,
                )
                .await;

            assert!(
                matches!(result, Ok(SamplerFailureRecovery::FlattenAndResubmit)),
                "the turn must continue on a flattened history, got {result:?}"
            );
            let after = actor.chat_state_handle.get_conversation().await;
            assert!(
                !xai_grok_sampling_types::conversation::needs_flattening(&after),
                "nothing the model can refuse may be left: {after:?}"
            );
            let text: String = after
                .iter()
                .filter_map(|i| match i {
                    ConversationItem::Assistant(a) => Some(a.content.as_ref().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("weighing the options"),
                "the thinking text must survive the conversion: {text}"
            );
            assert!(
                text.contains("read_file"),
                "the tool call must survive as text: {text}"
            );
        })
        .await;
}

/// A second rejection on an already-flat history has nothing left to convert.
/// Resubmitting the same bytes forever is the failure this bound prevents, so
/// the turn ends and reports what the model actually said.
#[tokio::test(flavor = "current_thread")]
async fn a_rejection_with_nothing_left_to_convert_is_terminal() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _) = mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor =
                Arc::new(create_test_actor(1_000, 1_000_000, 85, gateway_tx, persistence_tx).await);
            actor.chat_state_handle.replace_conversation(vec![
                ConversationItem::user("hello"),
                ConversationItem::Assistant(AssistantItem {
                    content: "hi".into(),
                    tool_calls: Vec::new(),
                    model_id: None,
                    model_fingerprint: None,
                    reasoning_effort: None,
                }),
            ]);

            let result = actor
                .handle_sampling_failure(
                    encrypted_content_error(),
                    0,
                    transient_state(0, true),
                    false,
                    TurnParkState::Fresh,
                )
                .await;

            assert!(
                result.is_err(),
                "a flat history that is still refused must not loop: {result:?}"
            );
        })
        .await;
}

/// The session actor's half of a model switch: it converts the history in
/// place and reports what it converted.
#[tokio::test(flavor = "current_thread")]
async fn flatten_history_converts_in_place_and_reports_it() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _) = mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor =
                Arc::new(create_test_actor(1_000, 1_000_000, 85, gateway_tx, persistence_tx).await);
            actor
                .chat_state_handle
                .replace_conversation(history_with_provider_state());

            let report = actor.handle_flatten_history().await;

            assert_eq!(report.reasoning_to_text, 1);
            assert_eq!(report.tool_calls_to_text, 1);
            assert_eq!(report.tool_results_to_text, 1);
            let after = actor.chat_state_handle.get_conversation().await;
            assert!(!xai_grok_sampling_types::conversation::needs_flattening(
                &after
            ));
        })
        .await;
}

/// An already-flat history is left exactly as it is, and the report says so.
/// A model switch must not rewrite a conversation that needs no rewriting.
#[tokio::test(flavor = "current_thread")]
async fn flatten_history_leaves_a_plain_conversation_alone() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _) = mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _) = mpsc::unbounded_channel::<PersistenceMsg>();
            let actor =
                Arc::new(create_test_actor(1_000, 1_000_000, 85, gateway_tx, persistence_tx).await);
            let plain = vec![
                ConversationItem::user("hello"),
                ConversationItem::Assistant(AssistantItem {
                    content: "hi".into(),
                    tool_calls: Vec::new(),
                    model_id: Some("grok-4-fast".into()),
                    model_fingerprint: None,
                    reasoning_effort: None,
                }),
            ];
            actor.chat_state_handle.replace_conversation(plain.clone());

            let report = actor.handle_flatten_history().await;

            assert!(!report.changed_anything());
            let after = actor.chat_state_handle.get_conversation().await;
            assert_eq!(after.len(), plain.len());
            match &after[1] {
                ConversationItem::Assistant(a) => assert_eq!(
                    a.model_id.as_deref(),
                    Some("grok-4-fast"),
                    "an untouched message keeps its origin"
                ),
                other => panic!("expected an assistant message, got {other:?}"),
            }
        })
        .await;
}
