//! Fitting a request's output budget into the model's context window.
//!
//! A provider counts the requested output against the same window as the
//! prompt. So the output budget is not a free parameter of the request: it is
//! whatever the window has left. This module answers that question for the
//! request shape every backend converter reads, so no path can serialize a
//! body that is arithmetically impossible.
//!
//! The per-item estimate lives here too, because this is where the
//! `ConversationItem` type lives. `xai-chat-state` re-exports it, and its
//! exact tracked count (server usage plus the delta since) is the better
//! number where a caller has one — pass that to [`ConversationRequest::fit_output_budget`]
//! instead of the estimate.

use xai_token_estimation::BYTES_PER_TOKEN;

use super::{ContentPart, ConversationItem, ConversationRequest, ToolSpec, reasoning_item_text};

/// An output budget that did not fit, and what it was cut to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputBudgetClamp {
    /// What the request asked for.
    pub requested: u32,
    /// What it now asks for.
    pub applied: u32,
    /// The prompt size the cut was computed against.
    pub prompt_tokens: u64,
    /// The window both have to share.
    pub context_window: u64,
}

/// Bytes/4 estimate for one [`ConversationItem`].
///
/// Images count at [`xai_token_estimation::IMAGE_TOKEN_ESTIMATE`] each.
pub fn estimate_item_tokens(item: &ConversationItem) -> u64 {
    match item {
        ConversationItem::System(s) => xai_token_estimation::estimate_tokens(&s.content),
        ConversationItem::User(u) => {
            let mut bytes: usize = 0;
            let mut images: u64 = 0;
            for p in &u.content {
                match p {
                    ContentPart::Text { text } => bytes += text.len(),
                    ContentPart::Image { .. } => images += 1,
                }
            }
            (bytes as u64) / BYTES_PER_TOKEN + xai_token_estimation::estimate_image_tokens(images)
        }
        ConversationItem::Assistant(a) => {
            let bytes = a.content.len()
                + a.tool_calls
                    .iter()
                    .map(|tc| tc.arguments.len())
                    .sum::<usize>();
            (bytes as u64) / BYTES_PER_TOKEN
        }
        ConversationItem::ToolResult(tr) => xai_token_estimation::estimate_tokens(&tr.content),
        ConversationItem::BackendToolCall(b) => {
            xai_token_estimation::estimate_tokens(&b.text_summary())
        }
        ConversationItem::Reasoning(r) => {
            // An encrypted blob is base64 and does not tokenize 1:1, so it is
            // estimated at len/4 like the text beside it.
            let text_bytes = reasoning_item_text(r).len();
            let enc_bytes = r.encrypted_content.as_deref().map_or(0, str::len);
            ((text_bytes + enc_bytes) as u64) / BYTES_PER_TOKEN
        }
    }
}

/// Bytes/4 estimate of one tool definition as the wire carries it.
pub fn estimate_tool_spec_tokens(spec: &ToolSpec) -> u64 {
    let name = spec.name.len();
    let desc = spec.description.as_deref().map_or(0, str::len);
    let params = spec.parameters.to_string().len();
    ((name + desc + params) as u64) / BYTES_PER_TOKEN
}

impl ConversationRequest {
    /// Bytes/4 estimate of everything this request puts in the prompt: the
    /// conversation items and the tool definitions that ride with them.
    ///
    /// This is an estimate, not a count. A caller that tracks the provider's
    /// own reported usage should pass that number to
    /// [`Self::fit_output_budget`] instead.
    pub fn estimate_prompt_tokens(&self) -> u64 {
        let items: u64 = self.items.iter().map(estimate_item_tokens).sum();
        let tools: u64 = self.tools.iter().map(estimate_tool_spec_tokens).sum();
        items.saturating_add(tools)
    }

    /// Cut `max_output_tokens` down to what `context_window` has left after
    /// `prompt_tokens`, and report the cut.
    ///
    /// Returns `None` when the request already fits, when the window is
    /// unknown (`0`), or when the request names no output budget — the
    /// sampler's own default is applied before this runs, so `None` there
    /// means nothing bounds the output at all.
    pub fn fit_output_budget(
        &mut self,
        prompt_tokens: u64,
        context_window: u64,
    ) -> Option<OutputBudgetClamp> {
        let requested = self.max_output_tokens?;
        let applied = xai_token_estimation::fit_output_tokens(
            u64::from(requested),
            prompt_tokens,
            context_window,
        );
        let applied = u32::try_from(applied).unwrap_or(u32::MAX);
        if applied >= requested {
            return None;
        }
        self.max_output_tokens = Some(applied);
        Some(OutputBudgetClamp {
            requested,
            applied,
            prompt_tokens,
            context_window,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_budget(max_output_tokens: u32) -> ConversationRequest {
        ConversationRequest {
            max_output_tokens: Some(max_output_tokens),
            ..Default::default()
        }
    }

    /// The reported failure: 737_857 + 262_144 is one token over a 1M window.
    /// The request that goes out has to be the one that fits.
    #[test]
    fn an_output_budget_the_window_cannot_hold_is_cut_to_fit() {
        let mut req = request_with_budget(262_144);
        let clamp = req
            .fit_output_budget(737_857, 1_000_000)
            .expect("a request one token over the window must be cut");
        assert_eq!(clamp.requested, 262_144);
        assert_eq!(clamp.applied, 262_143);
        assert_eq!(req.max_output_tokens, Some(262_143));
        assert_eq!(clamp.prompt_tokens + u64::from(clamp.applied), 1_000_000);
    }

    #[test]
    fn a_request_that_fits_is_reported_unchanged() {
        let mut req = request_with_budget(262_144);
        assert!(req.fit_output_budget(100_000, 1_000_000).is_none());
        assert_eq!(req.max_output_tokens, Some(262_144));
    }

    #[test]
    fn an_unknown_window_or_unset_budget_changes_nothing() {
        let mut req = request_with_budget(262_144);
        assert!(req.fit_output_budget(737_857, 0).is_none());
        assert_eq!(req.max_output_tokens, Some(262_144));

        let mut req = ConversationRequest::default();
        assert!(req.fit_output_budget(737_857, 1_000_000).is_none());
        assert_eq!(req.max_output_tokens, None);
    }

    #[test]
    fn a_prompt_that_fills_the_window_asks_for_the_floor() {
        let mut req = request_with_budget(262_144);
        let clamp = req
            .fit_output_budget(1_200_000, 1_000_000)
            .expect("an over-window prompt must still cut the budget");
        assert_eq!(
            u64::from(clamp.applied),
            xai_token_estimation::MIN_OUTPUT_TOKENS
        );
    }

    #[test]
    fn the_prompt_estimate_counts_items_and_tool_definitions() {
        let mut req =
            ConversationRequest::from_items(vec![ConversationItem::user("x".repeat(4_000))]);
        let items_only = req.estimate_prompt_tokens();
        assert_eq!(items_only, 1_000);

        req.tools.push(ToolSpec {
            name: "t".repeat(4),
            description: Some("d".repeat(400)),
            parameters: serde_json::json!({}),
        });
        assert!(
            req.estimate_prompt_tokens() > items_only,
            "tool definitions ride in the prompt and must be counted"
        );
    }
}
