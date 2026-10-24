use super::*;

impl SessionActor {
    /// Summarize the thinking in `response`, keyed by its `stream_start_ms`.
    pub(crate) fn spawn_thinking_summary(
        self: &Arc<Self>,
        response: &xai_grok_sampling_types::ConversationResponse,
        stream_start_ms: Option<i64>,
    ) {
        use crate::session::helpers::thinking_summary as helpers;
        if !self.thinking_summaries_enabled {
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
        // The client draws a spinner until the summary or a failure arrives.
        self.send_thinking_summary_status(stream_start_ms, None);
        // One summarization request runs at a time per session.
        let previous = self.thinking_summary_tail.lock().unwrap().take();
        let actor = self.clone();
        let handle = tokio::task::spawn_local(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            if let Err(reason) = actor
                .generate_thinking_summary(thinking, stream_start_ms)
                .await
            {
                actor.send_thinking_summary_status(stream_start_ms, Some(reason));
            }
        });
        *self.thinking_summary_tail.lock().unwrap() = Some(handle);
    }

    fn send_thinking_summary_status(&self, stream_start_ms: i64, failed: Option<String>) {
        self.send_xai_notification_transient(XaiSessionUpdate::ThinkingSummaryStatus {
            stream_start_ms,
            failed,
        });
    }

    /// The error is the text the client shows in place of the summary.
    async fn generate_thinking_summary(
        &self,
        thinking: String,
        stream_start_ms: i64,
    ) -> Result<(), String> {
        use crate::session::helpers::thinking_summary as helpers;
        // Snapshot the session's earlier summaries before this call's own is recorded.
        let prior = self.thinking_summary_history.prior_for(stream_start_ms);
        let setup = match self.prepare_side_call("thinking_summary").await {
            Ok(setup) => setup,
            Err(e) => {
                tracing::warn!(error = %e, "thinking summary: no sampling client");
                return Err(format!("no sampling client: {e}"));
            }
        };
        let session_id = self.session_info.id.to_string();
        // A summary is a fragment of an answer's cost, not a reasoning task.
        let reasoning_effort = self.models_manager.reasoning_effort_for(
            crate::agent::reasoning_policy::ReasoningNeed::Off,
            &setup.model,
        );
        let request = ConversationRequest {
            items: vec![ConversationItem::user(
                helpers::thinking_summary_instruction(&thinking, &prior),
            )],
            model: Some(setup.model.clone()),
            reasoning_effort: Some(reasoning_effort),
            max_output_tokens: Some(helpers::THINKING_SUMMARY_MAX_OUTPUT_TOKENS),
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
                return Err(format!("model call failed: {e}"));
            }
        };
        let summary = helpers::clean_thinking_summary(&response.assistant_text());
        if summary.is_empty() {
            tracing::warn!("thinking summary: the model returned no text");
            return Err("the model returned no text".to_string());
        }
        // Record BEFORE the broadcast: the next call may draw on this.
        self.thinking_summary_history
            .record(stream_start_ms, summary.clone());
        self.send_xai_notification(XaiSessionUpdate::ThinkingSummary {
            stream_start_ms,
            summary,
        })
        .await;
        Ok(())
    }
}
