use super::*;

impl SessionActor {
    /// Summarize long thinking in `response`, keyed by its `stream_start_ms`.
    pub(crate) fn spawn_thinking_summary(
        self: &Arc<Self>,
        response: &xai_grok_sampling_types::ConversationResponse,
        stream_start_ms: Option<i64>,
    ) {
        use crate::session::helpers::thinking_summary as helpers;
        if !self.thinking_summaries_enabled || self.startup_hints.is_subagent {
            return;
        }
        let Some(stream_start_ms) = stream_start_ms else {
            tracing::warn!(
                "thinking summary: the response has no stream start, so nothing can key it"
            );
            return;
        };
        let Some(thinking) =
            helpers::summarizable_thinking(&helpers::response_thinking_text(response))
        else {
            return;
        };
        let actor = self.clone();
        tokio::task::spawn_local(async move {
            actor
                .generate_thinking_summary(thinking, stream_start_ms)
                .await;
        });
    }

    async fn generate_thinking_summary(&self, thinking: String, stream_start_ms: i64) {
        use crate::session::helpers::thinking_summary as helpers;
        let setup = match self.prepare_side_call("thinking_summary").await {
            Ok(setup) => setup,
            Err(e) => {
                tracing::warn!(error = %e, "thinking summary: no sampling client");
                return;
            }
        };
        let session_id = self.session_info.id.to_string();
        let request = ConversationRequest {
            items: vec![ConversationItem::user(
                helpers::thinking_summary_instruction(&thinking),
            )],
            model: Some(setup.model.clone()),
            x_grok_conv_id: Some(format!("thinking-summary-{}", uuid::Uuid::new_v4())),
            x_grok_session_id: Some(session_id),
            x_grok_agent_id: Some(xai_grok_telemetry::id::agent_id()),
            ..Default::default()
        };
        let response = match super::side_call::collect_aux_call(
            &setup.client,
            &request,
            "thinking-summary",
            |e, delay| tracing::debug!(error = %e, ?delay, "thinking summary: retrying"),
        )
        .await
        {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!(error = %e, "thinking summary: model call failed");
                return;
            }
        };
        let summary = helpers::clean_thinking_summary(&response.assistant_text());
        if summary.is_empty() {
            tracing::warn!("thinking summary: the model returned no text");
            return;
        }
        self.send_xai_notification(XaiSessionUpdate::ThinkingSummary {
            stream_start_ms,
            summary,
        })
        .await;
    }
}
