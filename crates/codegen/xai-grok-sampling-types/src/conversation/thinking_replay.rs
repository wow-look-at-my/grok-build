//! One policy for replayed thinking, shared by every wire builder.
//!
//! A reasoning item carries up to two things: words (`summary` or `content`)
//! and a model-bound blob (`encrypted_content`: a Messages signature or a
//! Responses encrypted body). The blob is verified against the model that
//! minted it. The words are not verified by anyone. This module decides, for
//! one item and one target model, which of the two goes on the wire.
//!
//! The rules, item on the left and target on the right. Same model, or a
//! model nobody recorded: `Native`. A blob with words, other model: `Text`.
//! A blob alone, other model: `Drop`. Words alone, other model that does not
//! sign: `Native`. Words alone, other model that signs: `Drop`.
//!
//! Two models that both sign cannot read each other's blob, so the static
//! answer for that pair is `Text`. A blob that a same-model check misjudges
//! still reaches the server. The server's rejection then steps
//! [`ThinkingReplay`] down: `Native`, then `TextOnly`, then `Scrubbed`.

use super::*;

/// How far a request may carry replayed thinking. Each level is the fallback
/// for a rejection of the level above it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingReplay {
    /// Blobs ride as typed thinking where the table above allows it.
    #[default]
    Native,
    /// Every block rides as plain assistant text. No blob reaches the wire.
    TextOnly,
    /// No thinking reaches the model at all.
    Scrubbed,
}

impl ThinkingReplay {
    /// The next fallback, or `None` at the last one.
    pub fn degraded(self) -> Option<Self> {
        match self {
            Self::Native => Some(Self::TextOnly),
            Self::TextOnly => Some(Self::Scrubbed),
            Self::Scrubbed => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::TextOnly => "text_only",
            Self::Scrubbed => "scrubbed",
        }
    }
}

/// What one reasoning item does on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingDisposition {
    /// The item goes out as the backend's typed thinking, blob included.
    Native,
    /// The words go out as ordinary assistant text. The blob does not.
    Text,
    /// Nothing goes out.
    Drop,
}

/// Whether a reasoning item carries a model-bound blob.
pub fn is_signed(r: &rs::ReasoningItem) -> bool {
    r.encrypted_content
        .as_deref()
        .is_some_and(|blob| !blob.is_empty())
}

/// Whether a reasoning item carries words a model can read as text.
pub fn has_text(r: &rs::ReasoningItem) -> bool {
    !reasoning_item_text(r).trim().is_empty()
}

/// Whether a blob minted by `origin` is readable by `target`. An alias and
/// the dated snapshot it answers as are one model (`claude-opus-5` /
/// `claude-opus-5-20260101`). A gateway's routing prefix
/// (`anthropic/claude-opus-5`) is not part of the name.
pub fn same_model(origin: &str, target: &str) -> bool {
    fn normalize(model: &str) -> &str {
        let model = model.rsplit('/').next().unwrap_or(model);
        model.strip_suffix("-latest").unwrap_or(model)
    }

    let (origin, target) = (normalize(origin), normalize(target));
    if origin.eq_ignore_ascii_case(target) {
        return true;
    }
    let (long, short) = if origin.len() > target.len() {
        (origin, target)
    } else {
        (target, origin)
    };
    let Some(prefix) = long.as_bytes().get(..short.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(short.as_bytes()) {
        return false;
    }
    match long.as_bytes()[short.len()..].split_first() {
        Some((b'-', date)) => !date.is_empty() && date.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// The model behind a reasoning item. The streaming layer emits each
/// `Reasoning` as the sibling of the `Assistant` item that follows it. A
/// `User` or `System` item closes the turn, so reasoning with no assistant
/// behind it has no recorded origin.
pub fn reasoning_origin_model(items: &[ConversationItem], reasoning_idx: usize) -> Option<&str> {
    items[reasoning_idx + 1..]
        .iter()
        .find_map(|item| match item {
            ConversationItem::Assistant(a) => Some(a.model_id.as_deref()),
            ConversationItem::User(_) | ConversationItem::System(_) => Some(None),
            _ => None,
        })
        .flatten()
}

/// Whether `target` signs its thinking, judged by what it already put in this
/// conversation. Nothing in the request declares which models sign, so a
/// block this model signed earlier is the one piece of evidence there is.
pub fn target_signs_thinking(items: &[ConversationItem], target: &str) -> bool {
    items.iter().enumerate().any(|(idx, item)| {
        matches!(item, ConversationItem::Reasoning(r) if is_signed(r))
            && reasoning_origin_model(items, idx).is_some_and(|origin| same_model(origin, target))
    })
}

/// The replay decision for one request, computed once and asked per item.
#[derive(Debug, Clone)]
pub struct ThinkingReplayPlan {
    target: Option<String>,
    target_signs: bool,
    level: ThinkingReplay,
}

impl ThinkingReplayPlan {
    /// `target_known_to_sign` is what the backend knows on its own, such as a
    /// Claude id on the Messages backend. The conversation's own evidence is
    /// added to it.
    pub fn new(req: &ConversationRequest, target_known_to_sign: bool) -> Self {
        let target = req.model.clone();
        let target_signs = target_known_to_sign
            || target
                .as_deref()
                .is_some_and(|t| target_signs_thinking(&req.items, t));
        Self {
            target,
            target_signs,
            level: req.thinking_replay,
        }
    }

    pub fn level(&self) -> ThinkingReplay {
        self.level
    }

    /// The disposition of the reasoning item at `idx`.
    pub fn disposition(
        &self,
        items: &[ConversationItem],
        idx: usize,
        r: &rs::ReasoningItem,
    ) -> ThinkingDisposition {
        let words = has_text(r);
        let text_or_drop = || {
            if words {
                ThinkingDisposition::Text
            } else {
                ThinkingDisposition::Drop
            }
        };
        match self.level {
            ThinkingReplay::Scrubbed => return ThinkingDisposition::Drop,
            ThinkingReplay::TextOnly => return text_or_drop(),
            ThinkingReplay::Native => {}
        }
        let (Some(target), Some(origin)) =
            (self.target.as_deref(), reasoning_origin_model(items, idx))
        else {
            return ThinkingDisposition::Native;
        };
        if same_model(origin, target) {
            return ThinkingDisposition::Native;
        }
        if is_signed(r) {
            return text_or_drop();
        }
        if self.target_signs {
            ThinkingDisposition::Drop
        } else {
            ThinkingDisposition::Native
        }
    }

    /// Whether the reasoning at `idx` is left behind by this plan.
    pub fn is_foreign(
        &self,
        items: &[ConversationItem],
        idx: usize,
        r: &rs::ReasoningItem,
    ) -> bool {
        self.disposition(items, idx, r) != ThinkingDisposition::Native
    }
}

/// The plain-text form of a reasoning item's words.
pub fn thinking_as_text(r: &rs::ReasoningItem) -> String {
    format!("<thinking>\n{}\n</thinking>", reasoning_item_text(r))
}

/// Rewrite a conversation to one replay level, for a caller that sends items
/// without a [`ConversationRequest`]. `Native` returns the items untouched.
/// `TextOnly` turns each reasoning item with words into an assistant text
/// item and drops the rest. `Scrubbed` drops every reasoning item.
pub fn apply_thinking_replay(
    items: Vec<ConversationItem>,
    level: ThinkingReplay,
) -> Vec<ConversationItem> {
    if level == ThinkingReplay::Native {
        return items;
    }
    items
        .into_iter()
        .filter_map(|item| match item {
            ConversationItem::Reasoning(r) => {
                if level == ThinkingReplay::TextOnly && has_text(&r) {
                    Some(ConversationItem::Assistant(AssistantItem {
                        content: Arc::<str>::from(thinking_as_text(&r)),
                        tool_calls: Vec::new(),
                        model_id: None,
                        model_fingerprint: None,
                        reasoning_effort: None,
                    }))
                } else {
                    None
                }
            }
            other => Some(other),
        })
        .collect()
}

/// Whether a provider's rejection names replayed thinking it could not take:
/// a Messages signature it cannot verify, or a Responses `encrypted_content`
/// it cannot decrypt. Either one is answered by stepping the replay level
/// down, on every path that sends history.
pub fn names_replayed_thinking(message: &str) -> bool {
    if message.contains("encrypted_content") {
        return true;
    }
    let message = message.to_ascii_lowercase();
    message.contains("signature") && message.contains("thinking")
}

impl ConversationRequest {
    /// Step the replay level down one fallback. Answers `false` when the
    /// request carries no reasoning, or is already at the last level, so the
    /// caller reports the rejection instead of resending the same body.
    pub fn degrade_thinking_replay(&mut self) -> bool {
        if !self
            .items
            .iter()
            .any(|item| matches!(item, ConversationItem::Reasoning(_)))
        {
            return false;
        }
        match self.thinking_replay.degraded() {
            Some(next) => {
                self.thinking_replay = next;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    fn turn(model: Option<&str>, r: ConversationItem) -> Vec<ConversationItem> {
        vec![
            ConversationItem::user("q1"),
            r,
            ConversationItem::Assistant(AssistantItem {
                content: "answer".into(),
                tool_calls: vec![],
                model_id: model.map(str::to_owned),
                model_fingerprint: None,
                reasoning_effort: None,
            }),
            ConversationItem::user("q2"),
        ]
    }

    fn disposition_for(
        origin: Option<&str>,
        r: ConversationItem,
        target: Option<&str>,
        target_known_to_sign: bool,
        level: ThinkingReplay,
    ) -> ThinkingDisposition {
        let mut req = ConversationRequest::from_items(turn(origin, r));
        req.model = target.map(str::to_owned);
        req.thinking_replay = level;
        let plan = ThinkingReplayPlan::new(&req, target_known_to_sign);
        let ConversationItem::Reasoning(item) = &req.items[1] else {
            unreachable!()
        };
        plan.disposition(&req.items, 1, item)
    }

    fn signed_with_words() -> ConversationItem {
        reasoning_sibling("r1", "the words", Some("blob"))
    }

    fn signed_only() -> ConversationItem {
        reasoning_sibling("tco_1", "", Some("blob"))
    }

    fn words_only() -> ConversationItem {
        reasoning_sibling("r1", "the words", None)
    }

    /// The whole table from the module doc, one row per case.
    #[test]
    fn the_truth_table() {
        use ThinkingDisposition as D;
        use ThinkingReplay::Native;
        let cases: Vec<(&str, ThinkingDisposition, ThinkingDisposition)> = vec![
            // (name, got, want)
            (
                "raw -> raw: full transcript compatible",
                disposition_for(Some("grok-4"), words_only(), Some("grok-3"), false, Native),
                D::Native,
            ),
            (
                "raw -> same model",
                disposition_for(Some("grok-4"), words_only(), Some("grok-4"), false, Native),
                D::Native,
            ),
            (
                "raw -> encrypted model: scrub",
                disposition_for(
                    Some("grok-4"),
                    words_only(),
                    Some("claude-opus-5"),
                    true,
                    Native,
                ),
                D::Drop,
            ),
            (
                "encrypted with summary -> raw model: the summary replaces the blob",
                disposition_for(
                    Some("claude-opus-5"),
                    signed_with_words(),
                    Some("grok-4"),
                    false,
                    Native,
                ),
                D::Text,
            ),
            (
                "encrypted without summary -> raw model: scrub",
                disposition_for(
                    Some("claude-opus-5"),
                    signed_only(),
                    Some("grok-4"),
                    false,
                    Native,
                ),
                D::Drop,
            ),
            (
                "encrypted -> same encrypted model: verbatim",
                disposition_for(
                    Some("claude-opus-5"),
                    signed_with_words(),
                    Some("claude-opus-5-20260101"),
                    true,
                    Native,
                ),
                D::Native,
            ),
            (
                "encrypted -> other encrypted model: the summary",
                disposition_for(
                    Some("claude-opus-5"),
                    signed_with_words(),
                    Some("claude-sonnet-5"),
                    true,
                    Native,
                ),
                D::Text,
            ),
            (
                "encrypted only -> other encrypted model: scrub",
                disposition_for(
                    Some("claude-opus-5"),
                    signed_only(),
                    Some("claude-sonnet-5"),
                    true,
                    Native,
                ),
                D::Drop,
            ),
            (
                "unknown origin: replayed as before",
                disposition_for(
                    None,
                    signed_with_words(),
                    Some("claude-opus-5"),
                    true,
                    Native,
                ),
                D::Native,
            ),
            (
                "unknown target: replayed as before",
                disposition_for(Some("grok-4"), signed_with_words(), None, false, Native),
                D::Native,
            ),
        ];
        for (name, got, want) in cases {
            assert_eq!(got, want, "{name}");
        }
    }

    /// The fallback levels override the table: `TextOnly` sends words and no
    /// blob even to the model that minted it, and `Scrubbed` sends nothing.
    #[test]
    fn the_fallback_levels_outrank_the_table() {
        use ThinkingDisposition as D;
        for (level, with_words, blob_only) in [
            (ThinkingReplay::TextOnly, D::Text, D::Drop),
            (ThinkingReplay::Scrubbed, D::Drop, D::Drop),
        ] {
            assert_eq!(
                disposition_for(
                    Some("claude-opus-5"),
                    signed_with_words(),
                    Some("claude-opus-5"),
                    true,
                    level
                ),
                with_words,
                "{level:?} with words"
            );
            assert_eq!(
                disposition_for(
                    Some("claude-opus-5"),
                    signed_only(),
                    Some("claude-opus-5"),
                    true,
                    level
                ),
                blob_only,
                "{level:?} blob only"
            );
        }
    }

    /// The conversation's own evidence makes a target a signing one: a block
    /// this model signed earlier means an unsigned block is refused later.
    #[test]
    fn a_signed_block_earlier_in_the_conversation_is_evidence_the_target_signs() {
        let items = vec![
            ConversationItem::user("q1"),
            reasoning_sibling("r1", "own thinking", Some("sig-1")),
            ConversationItem::assistant_with_model("first", "claude-opus-5"),
            ConversationItem::user("q2"),
            words_only(),
            ConversationItem::assistant_with_model("second", "grok-4"),
            ConversationItem::user("q3"),
        ];
        let req = ConversationRequest::from_items(items).with_model("claude-opus-5");
        let plan = ThinkingReplayPlan::new(&req, false);
        let ConversationItem::Reasoning(own) = &req.items[1] else {
            unreachable!()
        };
        let ConversationItem::Reasoning(foreign) = &req.items[4] else {
            unreachable!()
        };
        assert_eq!(
            plan.disposition(&req.items, 1, own),
            ThinkingDisposition::Native
        );
        assert_eq!(
            plan.disposition(&req.items, 4, foreign),
            ThinkingDisposition::Drop
        );
    }

    #[test]
    fn same_model_reads_aliases_snapshots_and_gateway_prefixes() {
        for (a, b, want) in [
            ("claude-opus-5", "claude-opus-5-20260101", true),
            ("claude-opus-5", "anthropic/claude-opus-5", true),
            ("claude-opus-5", "claude-opus-5-latest", true),
            ("claude-opus-5", "Claude-Opus-5", true),
            ("claude-opus-5", "claude-opus-5-mini", false),
            ("grok-4", "grok-4-fast", false),
            ("", "grok-4", false),
        ] {
            assert_eq!(same_model(a, b), want, "{a} vs {b}");
        }
    }

    /// The ladder steps `Native` to `TextOnly` to `Scrubbed` and then stops.
    /// A request with no reasoning has nothing to step down for.
    #[test]
    fn degrade_walks_the_ladder_once() {
        let mut req = ConversationRequest::from_items(turn(Some("grok-4"), signed_with_words()));
        assert_eq!(req.thinking_replay, ThinkingReplay::Native);
        assert!(req.degrade_thinking_replay());
        assert_eq!(req.thinking_replay, ThinkingReplay::TextOnly);
        assert!(req.degrade_thinking_replay());
        assert_eq!(req.thinking_replay, ThinkingReplay::Scrubbed);
        assert!(!req.degrade_thinking_replay(), "the ladder ends");

        let mut plain = ConversationRequest::from_items(vec![ConversationItem::user("q")]);
        assert!(
            !plain.degrade_thinking_replay(),
            "no reasoning means no level to step down"
        );
    }

    #[test]
    fn apply_thinking_replay_rewrites_items_per_level() {
        let items = vec![
            ConversationItem::user("q1"),
            signed_with_words(),
            signed_only(),
            ConversationItem::assistant_with_model("answer", "grok-4"),
        ];
        assert_eq!(
            apply_thinking_replay(items.clone(), ThinkingReplay::Native).len(),
            4
        );

        let text = apply_thinking_replay(items.clone(), ThinkingReplay::TextOnly);
        assert_eq!(text.len(), 3, "the blob-only item is gone: {text:?}");
        assert!(matches!(
            &text[1],
            ConversationItem::Assistant(a) if a.content.contains("<thinking>") && a.content.contains("the words") && a.model_id.is_none()
        ));

        let scrubbed = apply_thinking_replay(items, ThinkingReplay::Scrubbed);
        assert_eq!(scrubbed.len(), 2);
        assert!(
            !scrubbed
                .iter()
                .any(|i| matches!(i, ConversationItem::Reasoning(_)))
        );
    }

    #[test]
    fn names_replayed_thinking_matches_both_provider_shapes() {
        assert!(names_replayed_thinking(
            "messages.19.content.0: Invalid `signature` in `thinking` block"
        ));
        assert!(names_replayed_thinking(
            "Could not decrypt the provided encrypted_content."
        ));
        assert!(!names_replayed_thinking(
            "request signature verification failed"
        ));
        assert!(!names_replayed_thinking("Invalid model parameter"));
    }
}
