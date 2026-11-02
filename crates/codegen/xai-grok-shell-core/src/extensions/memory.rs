//! Memory extension wire types: the `x.ai/memory/*` method names, requests and responses.

use serde::{Deserialize, Serialize};

use super::notification::{MemoryDisabledReason, MemoryFileInfo};
pub const MEMORY_FLUSH_METHOD: &str = "x.ai/memory/flush";
pub const MEMORY_DREAM_METHOD: &str = "x.ai/memory/dream";
pub const MEMORY_REWRITE_METHOD: &str = "x.ai/memory/rewrite";
pub const MEMORY_LIST_METHOD: &str = "x.ai/memory/list";
pub const MEMORY_TOGGLE_METHOD: &str = "x.ai/memory/toggle";
pub const MEMORY_FORGET_METHOD: &str = "x.ai/memory/forget";

/// How a `/dream` run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDreamDisposition {
    /// Observations.
    Completed,
    /// Nothing was waiting to be consolidated.
    NoWork,
    /// The consolidation lease is held, by another session or by this session's automatic Dream.
    Busy,
    /// An interrupted earlier Dream was finished instead of starting a new one.
    Recovered,
    /// This attempt failed; the observations stay queued for the next Dream.
    RetryRequired,
    Failed,
    /// Shadow rollout: the plan ran but nothing was written.
    Shadow,
    Cancelled,
    /// Memory or manual Dream is turned off for this session.
    Disabled,
}

/// Longest failure text a `/dream` summary carries. A model error can echo the whole request.
pub const DREAM_FAILURE_DETAIL_LIMIT: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDreamResponse {
    pub disposition: MemoryDreamDisposition,
    pub observation_count: usize,
    pub topics_affected: usize,
    /// What failed, for `RetryRequired` and `Failed`. Without it the summary names no cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl MemoryDreamResponse {
    pub const fn new(disposition: MemoryDreamDisposition) -> Self {
        Self {
            disposition,
            observation_count: 0,
            topics_affected: 0,
            detail: None,
        }
    }

    /// Observations were written to topics.
    fn did_work(&self) -> bool {
        matches!(
            self.disposition,
            MemoryDreamDisposition::Completed | MemoryDreamDisposition::Recovered
        ) && self.topics_affected > 0
    }

    /// Combine this pass's outcome with a coalesced follow-up pass so the run reports the
    /// work it did. A later `NoWork` must not hide an earlier merge.
    pub fn then(self, next: Self) -> Self {
        match (self.did_work(), next.did_work()) {
            (true, true) => Self {
                disposition: next.disposition,
                observation_count: self.observation_count + next.observation_count,
                topics_affected: self.topics_affected + next.topics_affected,
                detail: next.detail,
            },
            (true, false) if next.disposition == MemoryDreamDisposition::NoWork => self,
            _ => next,
        }
    }

    /// The run did what.
    pub fn succeeded(&self) -> bool {
        matches!(
            self.disposition,
            MemoryDreamDisposition::Completed
                | MemoryDreamDisposition::NoWork
                | MemoryDreamDisposition::Recovered
                | MemoryDreamDisposition::Shadow
        )
    }

    /// One-line outcome for scrollback; shared by the pager and non-pager ACP clients.
    pub fn summary(&self) -> String {
        let outcome = self.outcome_phrase();
        let Some(detail) = self
            .detail
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        else {
            return outcome;
        };
        let detail = match detail.char_indices().nth(DREAM_FAILURE_DETAIL_LIMIT) {
            Some((cut, _)) => format!("{}…", &detail[..cut]),
            None => detail.to_owned(),
        };
        format!("{outcome} Cause: {detail}")
    }

    fn outcome_phrase(&self) -> String {
        match self.disposition {
            MemoryDreamDisposition::Completed if self.topics_affected > 0 => format!(
                "Dream merged {} into {}.",
                plural(self.observation_count, "observation"),
                plural(self.topics_affected, "topic")
            ),
            MemoryDreamDisposition::Completed => "Dream completed.".to_owned(),
            MemoryDreamDisposition::NoWork => "Nothing to consolidate.".to_owned(),
            MemoryDreamDisposition::Busy => {
                "Dream is already running; try again when it finishes.".to_owned()
            }
            MemoryDreamDisposition::Recovered if self.topics_affected > 0 => format!(
                "Dream finished an interrupted earlier run, updating {}.",
                plural(self.topics_affected, "topic")
            ),
            MemoryDreamDisposition::Recovered => {
                "Dream finished an interrupted earlier run.".to_owned()
            }
            MemoryDreamDisposition::RetryRequired => {
                "Dream did not finish; it will retry automatically.".to_owned()
            }
            MemoryDreamDisposition::Failed => "Dream failed.".to_owned(),
            MemoryDreamDisposition::Shadow => {
                "Dream ran in shadow mode; nothing was written.".to_owned()
            }
            MemoryDreamDisposition::Cancelled => "Dream cancelled.".to_owned(),
            MemoryDreamDisposition::Disabled => "Dream is turned off for this session.".to_owned(),
        }
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// How a `/flush` run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryFlushDisposition {
    /// Every finished or stopped turn is captured and indexed.
    Flushed,
    /// Capture failed but will be retried in the background.
    RetryRequired,
    Failed,
    TimedOut,
    /// Memory or capture is turned off for this session.
    Disabled,
    /// Legacy memory: another flush was already running.
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFlushResponse {
    /// `disposition == Flushed`; kept because the headless `--memory-flush` barrier reads it.
    pub flushed: bool,
    pub disposition: MemoryFlushDisposition,
    /// Last turn the flush covered (memory v2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_turn: Option<u32>,
}

impl MemoryFlushResponse {
    pub fn succeeded(&self) -> bool {
        self.disposition == MemoryFlushDisposition::Flushed
    }

    /// One-line outcome for scrollback; shared by the pager and non-pager ACP clients.
    pub fn summary(&self) -> String {
        match (self.disposition, self.through_turn) {
            (MemoryFlushDisposition::Flushed, Some(turn)) => {
                format!("Memory flushed through turn {turn}.")
            }
            (MemoryFlushDisposition::Flushed, None) => "Memory flushed.".to_owned(),
            (MemoryFlushDisposition::RetryRequired, _) => {
                "Memory flush did not finish; capture will retry in the background.".to_owned()
            }
            (MemoryFlushDisposition::Failed, _) => "Memory flush failed.".to_owned(),
            (MemoryFlushDisposition::TimedOut, _) => {
                "Memory flush timed out; capture continues in the background.".to_owned()
            }
            (MemoryFlushDisposition::Disabled, _) => {
                "Memory is turned off for this session.".to_owned()
            }
            (MemoryFlushDisposition::Busy, _) => {
                "Another memory flush is already running.".to_owned()
            }
        }
    }
}

/// Largest note `x.ai/memory/forget` will hash and delete, for both v2 and legacy stores.
pub const MEMORY_FORGET_MAX_FILE_BYTES: u64 = xai_grok_memory::MAX_FORGET_FILE_BYTES;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryListRequest {
    pub session_id: String,
}

/// What the `/memory` modal renders: the same payload as the `MemoryFiles` notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryListing {
    pub files: Vec<MemoryFileInfo>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<MemoryDisabledReason>,
    pub capture_enabled: bool,
    pub dream_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryToggleRequest {
    pub session_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToggleResponse {
    /// Outcome for the user (also covers "already on" and refusals).
    pub message: String,
    /// Authoritative state after the toggle; refusals leave it unchanged.
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<MemoryDisabledReason>,
    /// Files after the toggle, so an open modal can refresh without a second round trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<MemoryListing>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryForgetRequest {
    pub session_id: String,
    /// Absolute path as listed by `MemoryFiles`.
    pub path: String,
    /// BLAKE3 hex of the bytes the user previewed; the store refuses to delete anything else.
    pub expected_content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum MemoryForgetResponse {
    Forgotten {
        was_already_forgotten: bool,
    },
    Rejected {
        reason: MemoryForgetRejection,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryForgetRejection {
    MemoryDisabled,
    /// Manifests, files outside the store, or anything the store's access policy protects.
    NotDeletable,
    /// The file no longer matches the previewed bytes.
    Changed,
    /// Dream holds the consolidation lease; retry once it finishes.
    DreamRunning,
    Failed,
}
/// Request body for `x.ai/memory/flush` and `x.ai/memory/dream` (snake_case, unlike the modal methods).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFlushRequest {
    pub session_id: String,
}
/// Request body for `x.ai/memory/rewrite`: a raw `/remember` note and the conversation summary the
/// model resolves references against.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRewriteRequest {
    pub session_id: String,
    pub raw_text: String,
    pub context_summary: String,
}

/// Response body for `x.ai/memory/rewrite`: the note as Markdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRewriteResponse {
    pub rewritten: String,
}
#[cfg(test)]
mod tests {
    use super::*;

    fn dream(
        disposition: MemoryDreamDisposition,
        observations: usize,
        topics: usize,
    ) -> MemoryDreamResponse {
        MemoryDreamResponse {
            disposition,
            observation_count: observations,
            topics_affected: topics,
            detail: None,
        }
    }

    #[test]
    fn coalesced_dream_passes_keep_the_work_done() {
        use MemoryDreamDisposition::*;
        let merged = dream(Completed, 7, 3);
        assert_eq!(merged.clone().then(dream(NoWork, 0, 0)), merged);
        assert_eq!(
            merged.clone().then(dream(Completed, 2, 1)),
            dream(Completed, 9, 4)
        );
        assert_eq!(
            merged.then(dream(RetryRequired, 2, 0)).disposition,
            RetryRequired
        );
        assert_eq!(
            dream(Cancelled, 0, 0).then(dream(NoWork, 0, 0)).disposition,
            NoWork
        );
    }

    #[test]
    fn a_retry_summary_names_its_cause() {
        let mut response = dream(MemoryDreamDisposition::RetryRequired, 3, 0);
        assert_eq!(
            response.summary(),
            "Dream did not finish; it will retry automatically."
        );
        response.detail = Some("invalid Dream plan: missing field `evidence`".to_owned());
        assert_eq!(
            response.summary(),
            "Dream did not finish; it will retry automatically. \
             Cause: invalid Dream plan: missing field `evidence`"
        );
    }

    #[test]
    fn a_long_cause_is_cut_on_a_character_boundary() {
        let mut response = dream(MemoryDreamDisposition::Failed, 0, 0);
        response.detail = Some("é".repeat(DREAM_FAILURE_DETAIL_LIMIT + 50));
        let summary = response.summary();
        let cause = summary.strip_prefix("Dream failed. Cause: ").unwrap();
        assert_eq!(cause.chars().count(), DREAM_FAILURE_DETAIL_LIMIT + 1);
        assert!(cause.ends_with('…'));
    }

    #[test]
    fn a_response_without_a_cause_keeps_its_wire_shape() {
        let json = serde_json::to_value(dream(MemoryDreamDisposition::NoWork, 0, 0)).unwrap();
        assert!(json.get("detail").is_none());
        let parsed: MemoryDreamResponse = serde_json::from_value(serde_json::json!({
            "disposition": "retry_required",
            "observation_count": 1,
            "topics_affected": 0,
        }))
        .unwrap();
        assert_eq!(parsed.detail, None);
    }
}
