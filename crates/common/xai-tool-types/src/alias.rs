//! A field its sender may name more than one way.

use std::error::Error;
use std::fmt;

/// The keys one field is read from: the canonical one first, then every
/// alias.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aliases {
    /// The key this type writes, and the one the rest of the code names.
    pub canonical: &'static str,
    /// Every other key the field is accepted under.
    pub aliases: &'static [&'static str],
}

impl Aliases {
    pub const fn new(canonical: &'static str, aliases: &'static [&'static str]) -> Self {
        Self { canonical, aliases }
    }

    /// Every key the field is read from, canonical first.
    pub fn keys(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.canonical).chain(self.aliases.iter().copied())
    }

    /// Reduce the values read under each key to the value the field holds.
    /// `values` holds one entry per key of [`keys`](Self::keys), in that
    /// order; a key absent from the input contributes `None`. The first value
    /// present is the result. A later key carrying a value equal to it is the
    /// same statement twice and is accepted. A later key carrying a different
    /// value contradicts the first, which no reader may resolve silently.
    pub fn fold<T>(&self, values: Vec<Option<T>>) -> Result<Option<T>, AliasConflict>
    where
        T: PartialEq + std::fmt::Debug,
    {
        let wanted = self.keys().count();
        if values.len() != wanted {
            return Err(AliasConflict::ShapeMismatch {
                canonical: self.canonical,
                keys: wanted,
                values: values.len(),
            });
        }

        let mut held: Option<(&'static str, T)> = None;
        for (key, value) in self.keys().zip(values) {
            let Some(value) = value else { continue };
            match &held {
                None => held = Some((key, value)),
                Some((from, prior)) => {
                    if prior != &value {
                        return Err(AliasConflict::DifferingValues {
                            canonical: self.canonical,
                            first_key: *from,
                            first_value: format!("{prior:?}"),
                            second_key: key,
                            second_value: format!("{value:?}"),
                        });
                    }
                }
            }
        }
        Ok(held.map(|(_, value)| value))
    }
}

/// What went wrong folding one field's key spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasConflict {
    /// Keys carried different values, so the input disagrees with itself.
    DifferingValues {
        canonical: &'static str,
        first_key: &'static str,
        first_value: String,
        second_key: &'static str,
        second_value: String,
    },
    /// The shadow struct named a different number of keys than the
    /// [`Aliases`] lists.
    ShapeMismatch {
        canonical: &'static str,
        keys: usize,
        values: usize,
    },
}

impl fmt::Display for AliasConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DifferingValues {
                canonical,
                first_key,
                first_value,
                second_key,
                second_value,
            } => write!(
                f,
                "field `{canonical}` was sent twice with different values: \
                 `{first_key}` = {first_value} and `{second_key}` = {second_value}"
            ),
            Self::ShapeMismatch {
                canonical,
                keys,
                values,
            } => write!(
                f,
                "field `{canonical}` reads {keys} keys but {values} values were offered"
            ),
        }
    }
}

impl Error for AliasConflict {}

/// One field a wire reader accepts under more than one key.
#[derive(Debug, Clone, Copy)]
pub struct WireAlias {
    /// Crate-relative path of the file that folds these keys.
    pub file: &'static str,
    /// The type whose shadow folds them.
    pub ty: &'static str,
    /// The key the type writes.
    pub canonical: &'static str,
    /// The keys it also accepts.
    pub aliases: &'static [&'static str],
}

/// Every field on an untrusted path that reads more than one key spelling,
/// folded through [`Aliases`]. The drift test in this module reads this table
/// against the source. A field that goes back to a bare `#[serde(alias)]`
/// shows up as an unclassified alias, and so does one newly added.
pub const WIRED: &[WireAlias] = &[
    WireAlias {
        file: "crates/codegen/xai-grok-sampling-types/src/types.rs",
        ty: "ChatChunkDelta",
        canonical: "reasoning_content",
        aliases: &["reasoning"],
    },
    WireAlias {
        // A campaign patch is merged into the same TOML value `Config` is read from and names any field.
        file: "crates/codegen/xai-grok-telemetry/src/config.rs",
        ty: "TelemetryConfig",
        canonical: "otel_protocol",
        aliases: &["otel_transport"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/agent/config.rs",
        ty: "EndpointsConfig",
        canonical: "models_list_url",
        aliases: &["models_endpoint"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-sampling-types/src/messages.rs",
        ty: "MessagesUsage",
        canonical: "cost_in_usd_ticks",
        aliases: &["cost_usd_ticks"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-sampling-types/src/messages.rs",
        ty: "MessageDeltaUsage",
        canonical: "cost_in_usd_ticks",
        aliases: &["cost_usd_ticks"],
    },
    // One entry covers both the `Image` and the `Resource` block: they read the
    // same key pair through the same [`ContentBlock::MIME_TYPE_KEYS`].
    WireAlias {
        file: "crates/common/xai-tool-runtime/src/tool.rs",
        ty: "ContentBlock",
        canonical: "mime_type",
        aliases: &["mimeType"],
    },
    WireAlias {
        file: "crates/common/xai-tool-types/src/task.rs",
        ty: "TaskOutputToolInput",
        canonical: "task_ids",
        aliases: &["task_id"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/grok_build/ask_user_question/mod.rs",
        ty: "Question",
        canonical: "multiSelect",
        aliases: &["multi_select"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/rewind.rs",
        ty: "RewindSessionRequest",
        canonical: "session_id",
        aliases: &["sessionId"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/rewind.rs",
        ty: "RewindSessionRequest",
        canonical: "target_prompt_index",
        aliases: &["targetPromptIndex"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/rewind.rs",
        ty: "RewindSessionRequest",
        canonical: "target_response_id",
        aliases: &["targetResponseId"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/rewind.rs",
        ty: "RewindPointsRequest",
        canonical: "session_id",
        aliases: &["sessionId"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/hooks.rs",
        ty: "ClientHookResponse",
        canonical: "systemMessage",
        aliases: &["reason"],
    },
    // The container renames to camelCase, so `sessionId` is the key this type
    // writes and `session_id` is the one it also accepts.
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/debug.rs",
        ty: "DebugTriggerParams",
        canonical: "sessionId",
        aliases: &["session_id"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-login/src/oidc/protocol.rs",
        ty: "MinimalClaims",
        canonical: "principal_type",
        aliases: &["principalType"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-login/src/oidc/protocol.rs",
        ty: "MinimalClaims",
        canonical: "principal_id",
        aliases: &["principalId"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-login/src/oidc/protocol.rs",
        ty: "PrincipalIdClaim",
        canonical: "principal_id",
        aliases: &["principalId"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-login/src/oidc/protocol.rs",
        ty: "IdTokenClaims",
        canonical: "first_name",
        aliases: &["given_name"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-login/src/oidc/protocol.rs",
        ty: "IdTokenClaims",
        canonical: "last_name",
        aliases: &["family_name"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/session/acp_types.rs",
        ty: "CompactConversationRequest",
        canonical: "session_id",
        aliases: &["sessionId"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/session/acp_types.rs",
        ty: "CompactConversationRequest",
        canonical: "user_context",
        aliases: &["userContext"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/session/acp_types.rs",
        ty: "ClientFeedbackInput",
        canonical: "turn_number",
        aliases: &["turnNumber"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/session/acp_types.rs",
        ty: "ClientFeedbackInput",
        canonical: "request_trace_upload_token",
        aliases: &["requestTraceUploadToken"],
    },
    // A struct variant cannot take `try_from`, so a variant-level
    // `deserialize_with` reads the variant through its own shadow.
    WireAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/notification.rs",
        ty: "SessionUpdate",
        canonical: "agentAddress",
        aliases: &["agent_address"],
    },
    // Both request types share one `Aliases`. `CreateWorktreeFromWorktreeRequest`
    // in xai-grok-workspace deserializes `from` the second one.
    WireAlias {
        file: "crates/codegen/xai-grok-workspace-types/src/rpc/worktree.rs",
        ty: "CreateWorktreeRequest",
        canonical: "groveWorktree",
        aliases: &["nfsWorktree", "nfs_worktree"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-workspace-types/src/rpc/worktree.rs",
        ty: "CreateWorktreeFromWorktreeRequestWire",
        canonical: "groveWorktree",
        aliases: &["nfsWorktree", "nfs_worktree"],
    },
    // `RemoteSettings` has too many fields for a shadow struct. Its
    // `Deserialize` folds the keys on the buffered object and then calls the
    // derived reader.
    WireAlias {
        file: "crates/codegen/xai-grok-config-types/src/lib.rs",
        ty: "RemoteSettings",
        canonical: "grove_worktree",
        aliases: &["nfs_worktree"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindPointInfo",
        canonical: "prompt_index",
        aliases: &["promptIndex"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindPointInfo",
        canonical: "created_at",
        aliases: &["createdAt"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindPointInfo",
        canonical: "num_file_snapshots",
        aliases: &["numFileSnapshots"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindPointInfo",
        canonical: "prompt_preview",
        aliases: &["promptPreview"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindPointInfo",
        canonical: "has_file_changes",
        aliases: &["hasFileChanges"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindPointsResponse",
        canonical: "rewind_points",
        aliases: &["rewindPoints"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindResponse",
        canonical: "target_prompt_index",
        aliases: &["targetPromptIndex"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindResponse",
        canonical: "reverted_files",
        aliases: &["revertedFiles"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindResponse",
        canonical: "clean_files",
        aliases: &["cleanFiles"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindResponse",
        canonical: "prompt_text",
        aliases: &["promptText"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-pager/src/views/rewind.rs",
        ty: "RewindConflictInfo",
        canonical: "conflict_type",
        aliases: &["conflictType"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-config-types/src/lib.rs",
        ty: "CampaignOverride",
        canonical: "id",
        aliases: &["campaign_id"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-config-types/src/mcp.rs",
        ty: "McpServerTransportConfig",
        canonical: "url",
        aliases: &["urlTemplate", "url_template"],
    },
    WireAlias {
        file: "crates/codegen/xai-grok-config-types/src/mcp.rs",
        ty: "McpSetupConfig",
        canonical: "variables",
        aliases: &["values"],
    },
];

/// A field that reads more than one key spelling and stays on a bare
/// `#[serde(alias)]`, because every byte it reads came out of a file.
#[derive(Debug, Clone, Copy)]
pub struct LocalAlias {
    /// Crate-relative path of the file that declares the alias.
    pub file: &'static str,
    /// The type whose field carries it.
    pub ty: &'static str,
    /// The key this type writes.
    pub canonical: &'static str,
    /// The keys it also accepts.
    pub aliases: &'static [&'static str],
    /// Why the input is this program's own.
    pub why: &'static str,
}

/// Every locally-written field that keeps a bare `#[serde(alias)]`.
pub const LOCAL: &[LocalAlias] = &[
    LocalAlias {
        file: "crates/codegen/xai-grok-shell/src/agent/config.rs",
        ty: "ConfigModelOverride",
        canonical: "compactions_remaining",
        aliases: &["send_compactions_remaining"],
        why: "the `[model.<id>]` TOML table, whose both-keys config \
              `config_model_override_parse::ALIASES` already resolves and warns \
              before serde sees it",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-shell/src/session/persistence.rs",
        ty: "PendingCwdSwitchReminder",
        canonical: "destination_cwd",
        aliases: &["cwd"],
        why: "a session summary this shell writes and re-reads; \
              `x.ai/session/list` only ever serializes it",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-sampling-types/src/conversation.rs",
        ty: "AssistantItem",
        canonical: "model_fingerprint",
        aliases: &["system_fingerprint"],
        why: "the conversation JSONL this program writes; every reader is a file \
              reader, and provider bodies parse into the wire types, which \
              `conversation/responses.rs` reads as JSON",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-config/src/campaigns.rs",
        ty: "CampaignMeta",
        canonical: "id",
        aliases: &["campaign_id"],
        why: "the `[[campaigns]]` TOML layers (requirements, user, managed, \
              system_managed), all files on disk; the remote layer's JSON \
              sibling is `CampaignOverride`",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "extensions",
        aliases: &["extensionToLanguage", "extensionToLanguageId"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "initialization_options",
        aliases: &["initializationOptions"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "workspace_folder",
        aliases: &["workspaceFolder"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "workspace_open",
        aliases: &["workspaceOpen"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "startup_timeout",
        aliases: &["startupTimeout"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "shutdown_timeout",
        aliases: &["shutdownTimeout"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "restart_on_crash",
        aliases: &["restartOnCrash"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-grok-tools/src/implementations/lsp/config.rs",
        ty: "LspServerConfig",
        canonical: "max_restarts",
        aliases: &["maxRestarts"],
        why: "`lsp.json` / `.lsp.json`, a config file this program reads from \
              disk and never receives",
    },
    LocalAlias {
        file: "crates/codegen/xai-fast-worktree/src/overlay/snapshot.rs",
        ty: "OverlayMetadata",
        canonical: "snapshot_root",
        aliases: &["snapshot_upper"],
        why: "`.fast-worktree-meta.json`, written by this program for its own \
              crash recovery; the alias reads a pre-rename file on disk",
    },
];

/// A `#[serde(alias)]` on an enum variant.
#[derive(Debug, Clone, Copy)]
pub struct EnumVariantAlias {
    /// Crate-relative path of the file that declares the enum.
    pub file: &'static str,
    /// The enum whose variants carry aliases.
    pub ty: &'static str,
    /// Every alias string the enum accepts on its variants.
    pub aliases: &'static [&'static str],
    /// Why no single object can carry some of these spellings.
    pub why: &'static str,
}

/// Every enum whose variants accept more than one name.
pub const ENUM_VARIANTS: &[EnumVariantAlias] = &[
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-tools/src/types/tool.rs",
        ty: "ToolNamespace",
        aliases: &[
            "GrokBuild",
            "GrokBuildConcise",
            "GrokBuildHashline",
            "OpenCode",
            "open_code",
            "MCP",
        ],
        why: "a namespace is one string key, so a value carrying two spellings \
              of it is a type error before a duplicate field",
    },
    EnumVariantAlias {
        file: "crates/common/xai-tool-types/src/task.rs",
        ty: "SubagentCapabilityMode",
        aliases: &[
            "readonly",
            "readOnly",
            "read_only",
            "ReadOnly",
            "readwrite",
            "readWrite",
            "read_write",
            "ReadWrite",
            "Execute",
            "EXECUTE",
            "All",
            "ALL",
        ],
        why: "a mode is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/common/xai-tool-types/src/task.rs",
        ty: "SubagentIsolationMode",
        aliases: &["None", "Worktree", "work_tree", "work-tree"],
        why: "an isolation mode is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-workspace/src/permission/types.rs",
        ty: "ClientType",
        aliases: &["grok-shell", "grok_shell", "grok_tui", "grok_pager"],
        why: "a client type is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-login/src/model.rs",
        ty: "AuthMode",
        aliases: &["grok", "oidc"],
        why: "an auth mode is one string key, read from the auth.json this \
              program wrote as well",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-shell/src/session/goal_tracker.rs",
        ty: "GoalStatus",
        aliases: &["Active", "Paused", "BudgetLimited", "Complete"],
        why: "a status is one string key, and this enum's Deserialize is \
              hand-written over `from_wire_str`, which reads these spellings off \
              that string, so the derived alias table never runs at all",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-shell/src/session/acp_types.rs",
        ty: "RewindMode",
        aliases: &["code_only"],
        why: "a rewind mode is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-shell/src/extensions/hooks.rs",
        ty: "ClientHookDecision",
        aliases: &["block"],
        why: "a decision is one string key, and `#[serde(other)]` catches every \
              name this enum does not spell",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-config-types/src/permission.rs",
        ty: "ToolFilter",
        aliases: &["agentmessage"],
        why: "a tool filter is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-workspace/src/permission/types.rs",
        ty: "ToolFilter",
        aliases: &["agentmessage"],
        why: "a tool filter is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-feedback/src/taxonomy.rs",
        ty: "FeedbackFailureMode",
        aliases: &[
            "did_too_much",
            "gave_up_early",
            "ignored_direction",
            "wrong_or_made_up",
            "broke_something",
            "stuck_in_loop",
        ],
        why: "a failure mode is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-sampling-types/src/conversation.rs",
        ty: "SyntheticReason",
        aliases: &["parent_agent_message"],
        why: "a synthetic reason is one string key; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-tools/src/types/output.rs",
        ty: "ToolOutput",
        aliases: &["SendAgentMessage"],
        why: "a variant name is one key of the enum's tag, so one value names \
              one variant; see ToolNamespace",
    },
    EnumVariantAlias {
        file: "crates/codegen/xai-grok-tools/src/types/tool_io.rs",
        ty: "ToolInput",
        aliases: &["SendAgentMessage"],
        why: "a variant name is one key of the enum's tag, so one value names \
              one variant; see ToolNamespace",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_value_under_one_key_reads_as_that_value() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        assert_eq!(
            spelling
                .fold(vec![Some("We".to_owned()), None])
                .unwrap()
                .as_deref(),
            Some("We")
        );
        assert_eq!(
            spelling
                .fold(vec![None, Some("We".to_owned())])
                .unwrap()
                .as_deref(),
            Some("We")
        );
        assert_eq!(spelling.fold::<String>(vec![None, None]).unwrap(), None);
    }

    /// The whole point: a gateway that sends the same text twice is not an error.
    #[test]
    fn the_same_value_under_both_keys_reads_once_and_is_not_an_error() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        let folded = spelling
            .fold(vec![Some("We".to_owned()), Some("We".to_owned())])
            .expect("identical spellings must not conflict");
        assert_eq!(folded.as_deref(), Some("We"));
    }

    #[test]
    fn two_different_values_error_naming_both_keys() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        let err = spelling
            .fold(vec![Some("one".to_owned()), Some("two".to_owned())])
            .expect_err("conflicting spellings must not resolve silently");
        let message = err.to_string();
        assert!(message.contains("reasoning_content"), "{message}");
        assert!(message.contains("`reasoning`"), "{message}");
        assert!(message.contains("different values"), "{message}");
    }

    #[test]
    fn a_shadow_naming_the_wrong_number_of_keys_is_an_error_not_a_panic() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        let err = spelling
            .fold(vec![Some("We".to_owned())])
            .expect_err("one value for two keys is a bug");
        assert!(err.to_string().contains("reads 2 keys"), "{err}");
    }

    #[test]
    fn keys_lists_the_canonical_key_first() {
        let spelling = Aliases::new("cost_in_usd_ticks", &["cost_usd_ticks", "cost"]);
        assert_eq!(
            spelling.keys().collect::<Vec<_>>(),
            vec!["cost_in_usd_ticks", "cost_usd_ticks", "cost"]
        );
    }

    /// Case one of the drift this table guards: a field that went back to a bare
    /// `#[serde(alias)]` - its `Aliases` declaration gone, or the shadow no
    /// longer folding every key it names.
    #[test]
    fn every_wired_alias_is_folded_in_the_file_that_declares_it() {
        let root = workspace_root();
        for wired in WIRED {
            let source = read_table_file(&root, wired.file);
            let declared = format!(
                "Aliases::new(\"{}\", &[{}])",
                wired.canonical,
                wired
                    .aliases
                    .iter()
                    .map(|a| format!("\"{a}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            assert!(
                source.contains(&declared),
                "{} names no `Aliases` for `{}` on {}; write `{}` in it",
                wired.file,
                wired.canonical,
                wired.ty,
                declared
            );
            let written = alias_sites_in(&source);
            for alias in wired.aliases {
                assert!(
                    !written.iter().any(|found| found == alias),
                    "{} still reads `{}` on {} through a bare #[serde(alias)], \
                     which is the duplicate-field failure this table exists to \
                     prevent",
                    wired.file,
                    alias,
                    wired.ty
                );
            }
        }
    }

    /// A new wire-facing field is the case this exists to catch. A `LOCAL` or
    /// `ENUM_VARIANTS` entry whose file dropped the alias is the same failure
    /// wearing the opposite sign.
    #[test]
    fn every_alias_in_the_tree_is_classified() {
        let root = workspace_root();
        let found = scan_alias_sites(&root);

        // The scan is worth nothing if it found nothing. Every `LOCAL` and
        // `ENUM_VARIANTS` alias stays a bare `#[serde(alias)]`, so all of them
        // must appear in the scan - which is also what proves the scanner is
        // reading the tree rather than silently skipping it.
        let declared_elsewhere: usize = LOCAL
            .iter()
            .flat_map(|e| e.aliases.iter().map(move |a| (e.file, *a)))
            .chain(
                ENUM_VARIANTS
                    .iter()
                    .flat_map(|e| e.aliases.iter().map(move |a| (e.file, *a))),
            )
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert!(
            declared_elsewhere > 0,
            "LOCAL and ENUM_VARIANTS name no alias at all"
        );
        let scanned_bare: usize = found
            .iter()
            .filter(|(file, alias)| {
                LOCAL
                    .iter()
                    .any(|e| e.file == file && e.aliases.contains(&alias.as_str()))
                    || ENUM_VARIANTS
                        .iter()
                        .any(|e| e.file == file && e.aliases.contains(&alias.as_str()))
            })
            .count();
        assert_eq!(
            scanned_bare, declared_elsewhere,
            "the scan found {scanned_bare} of the {declared_elsewhere} aliases \
             LOCAL and ENUM_VARIANTS say are still bare; the scan is missing files"
        );

        let mut unclassified = Vec::new();
        let mut misclassified = Vec::new();

        for (file, alias) in &found {
            let claimed_local = LOCAL
                .iter()
                .any(|e| &e.file == file && e.aliases.contains(&alias.as_str()));
            let claimed_enum = ENUM_VARIANTS
                .iter()
                .any(|e| &e.file == file && e.aliases.contains(&alias.as_str()));
            let claimed_wired = WIRED
                .iter()
                .any(|e| &e.file == file && e.aliases.contains(&alias.as_str()));
            if claimed_wired {
                misclassified.push((file.clone(), alias.clone()));
            } else if !claimed_local && !claimed_enum {
                unclassified.push((file.clone(), alias.clone()));
            }
        }

        assert!(
            misclassified.is_empty(),
            "these fields are in WIRED, so they must be folded through an \
             `Aliases` rather than read through a bare #[serde(alias)]: \
             {misclassified:?}"
        );
        assert!(
            unclassified.is_empty(),
            "these #[serde(alias)] sites are classified nowhere: fold a wire-facing \
             one into WIRED, or record a config-file one in LOCAL or an enum-variant \
             one in ENUM_VARIANTS, each with its reason: {unclassified:?}"
        );

        for entry in LOCAL {
            let source = read_table_file(&root, entry.file);
            let written = alias_sites_in(&source);
            for alias in entry.aliases {
                assert!(
                    written.iter().any(|found| found == alias),
                    "LOCAL claims `{}` on {} in {}, recorded as local because \
                     {}, which no longer declares it",
                    alias,
                    entry.ty,
                    entry.file,
                    entry.why
                );
            }
        }
        for entry in ENUM_VARIANTS {
            let source = read_table_file(&root, entry.file);
            let written = alias_sites_in(&source);
            for alias in entry.aliases {
                assert!(
                    written.iter().any(|found| found == alias),
                    "ENUM_VARIANTS claims `{}` on {} in {}, recorded as an enum \
                     variant alias because {}, which no longer declares it",
                    alias,
                    entry.ty,
                    entry.file,
                    entry.why
                );
            }
        }
    }

    /// The tables describe one partition of the same set, so a field
    /// cannot be both a folded wire key and a tolerated config alias.
    #[test]
    fn the_alias_tables_never_claim_one_field_twice() {
        for wired in WIRED {
            for local in LOCAL {
                assert!(
                    !(local.file == wired.file
                        && local.canonical == wired.canonical
                        && local.aliases == wired.aliases),
                    "{}#{} is in both WIRED and LOCAL",
                    wired.file,
                    wired.canonical
                );
            }
            // Entries naming the same keys in one file on one type are one
            // entry's worth of information; the duplicate hides a table that
            // drifted. Types in one file may fold the same key pair.
            for other in WIRED {
                if std::ptr::eq(wired, other) || wired.file != other.file || wired.ty != other.ty {
                    continue;
                }
                assert!(
                    wired.canonical != other.canonical,
                    "{} lists `{}` on {} twice",
                    wired.file,
                    wired.canonical,
                    wired.ty
                );
            }
        }
    }

    /// A table entry whose file is gone is a claim about code that does not
    /// exist. This is how a rename silently un-classifies a field.
    #[test]
    fn every_table_entry_names_a_file_that_still_exists() {
        let root = workspace_root();
        for file in WIRED
            .iter()
            .map(|e| e.file)
            .chain(LOCAL.iter().map(|e| e.file))
            .chain(ENUM_VARIANTS.iter().map(|e| e.file))
        {
            let path = root.join(file);
            assert!(
                path.is_file(),
                "{file} is named by an alias table and is not in the tree"
            );
        }
    }

    /// The scanner is the drift test's only eye. It gets its own: it reads an
    /// attribute rustfmt spread over lines. It ignores the same text appearing
    /// in a doc comment or a string literal.
    #[test]
    fn the_scanner_reads_attributes_and_not_prose() {
        let source = r##"
//! `#[serde(alias = "in_a_doc_comment")]` accepts a second key.
use serde::Deserialize;

#[derive(Deserialize)]
pub struct One {
    #[serde(alias = "solo")]
    pub canonical_solo: String,
    #[serde(
        default,
        alias = "first",
        alias = "second"
    )]
    pub spread: String,
    #[serde(rename = "moved")]
    pub renamed_without_alias: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tagged {
    #[serde(alias = "VariantName")]
    VariantName,
}

fn finder(haystack: &str) -> usize {
    haystack.find("#[serde(alias = \"in_a_string_literal\")]").unwrap()
}

fn clap_too(#[arg(long, alias = "a_cli_flag")] _flag: bool) {}

#[test]
fn quotes_the_attribute_itself() {
    let needle = "#[serde(alias = \"in_a_test_string\")]";
    assert!(needle.len() > 1);
}
"##;
        let found = alias_sites_in_source("fixture.rs", source);
        let aliases: Vec<&str> = found.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            aliases,
            vec!["solo", "first", "second", "VariantName"],
            "the scan must read every attribute alias and nothing else"
        );
    }

    /// A test module's fixtures are not aliases a peer will send. A
    /// `#[cfg(test)]` that marks ONE item must not blind the scan to the rest of
    /// the file. Both halves are what `shipped_source` has to get right, so both
    /// are asserted here and not only through the workspace walk.
    #[test]
    fn the_scan_skips_test_modules_and_nothing_else() {
        let source = r#"
pub struct Early {
    #[serde(default, alias = "before_tests")]
    pub before: Option<String>,
}

#[cfg(test)]
mod tests {
    #[derive(serde::Deserialize)]
    struct Fixture {
        #[serde(default, alias = "in_a_test_module")]
        pub field: String,
    }
}

pub struct Late {
    #[serde(default, alias = "after_tests")]
    pub after: Option<String>,
}
"#;
        let aliases: Vec<String> = alias_sites_in_source("f.rs", &shipped_source(source))
            .into_iter()
            .map(|(_, alias)| alias)
            .collect();
        assert_eq!(
            aliases,
            vec!["before_tests", "after_tests"],
            "a test module is skipped and the code after it is not"
        );
    }

    /// A `#[cfg(test)]` and its `mod` on one line introduces a module as
    /// surely as those-line spelling. Skipping it depends on finding the
    /// braces from the attribute's own line.
    #[test]
    fn the_scan_skips_a_test_module_opened_on_the_attribute_line() {
        let source = r#"
pub struct Early {
    #[serde(default, alias = "before_tests")]
    pub before: Option<String>,
}

#[cfg(test)]
mod inline_tests {
    #[derive(serde::Deserialize)]
    struct Fixture {
        #[serde(default, alias = "in_a_test_module")]
        pub field: String,
    }
}

pub struct Late {
    #[serde(default, alias = "after_tests")]
    pub after: Option<String>,
}
"#;
        let one_line = "#[cfg(test)] mod inline_tests {";
        let folded = source.replace("#[cfg(test)]\nmod inline_tests {", one_line);
        let aliases: Vec<String> = alias_sites_in_source("f.rs", &shipped_source(&folded))
            .into_iter()
            .map(|(_, alias)| alias)
            .collect();
        assert_eq!(
            aliases,
            vec!["before_tests", "after_tests"],
            "the same-line spelling opens a module too, and its fixture alias is not shipped"
        );
    }

    /// A wire-facing field that goes back to a bare `#[serde(alias)]` is the
    /// regression the tables exist to catch: the file still folds nothing. The
    /// alias is still there to be read.
    #[test]
    fn a_wire_alias_that_reverts_to_bare_serde_is_reported() {
        let reverted = r#"
            #[serde(default, alias = "cost_usd_ticks")]
            pub cost_in_usd_ticks: Option<i64>,
"#;
        let found = alias_sites_in_source(
            "crates/codegen/xai-grok-sampling-types/src/messages.rs",
            reverted,
        );
        assert_eq!(
            found,
            vec![(
                "crates/codegen/xai-grok-sampling-types/src/messages.rs".to_owned(),
                "cost_usd_ticks".to_owned()
            )],
            "a reverted wire alias must surface from the scan"
        );
        let claimed_wired = WIRED.iter().any(|e| {
            found
                .iter()
                .any(|(file, alias)| &e.file == file && e.aliases.contains(&alias.as_str()))
        });
        assert!(
            claimed_wired,
            "the scan result must be one WIRED already answers for, which is what \
             makes the drift test call it a misclassification"
        );
    }

    fn read_table_file(root: &std::path::Path, file: &str) -> String {
        std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("{file} is not readable: {e}"))
    }

    /// Every `(file, alias)` pair that a `#[serde(... alias = "...")]` reads,
    /// across the workspace.
    fn scan_alias_sites(root: &std::path::Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in rust_files(&root.join("crates")) {
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            out.extend(alias_sites_in_source(&relative, &shipped_source(&source)));
        }
        out.sort();
        out.dedup();
        out
    }

    /// The file minus its test modules. A test module is full of
    /// `#[serde(alias = ...)]` written to exercise this scanner, and those
    /// are not aliases any peer will ever send. A `#[cfg(test)]` that carries
    /// one attribute of an otherwise-shipped item must not cut the rest of
    /// the file. The skip is brace-matched over the module the attribute
    /// introduces rather than everything after it. That module's `mod`
    /// keyword sits either on the attribute's own line or on the next
    /// non-blank one, and both spellings are skipped.
    fn shipped_source(source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let mut kept = String::new();
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index].trim();
            let attribute_alone = line == "#[cfg(test)]";
            let attribute_with_module =
                line.starts_with("#[cfg(test)]") && line.ends_with('{') && line.contains("mod ");
            if attribute_alone || attribute_with_module {
                let mut body = index;
                if attribute_alone {
                    body += 1;
                    while body < lines.len() && lines[body].trim().is_empty() {
                        body += 1;
                    }
                }
                if attribute_with_module
                    || (body < lines.len() && lines[body].trim_start().starts_with("mod "))
                {
                    let mut depth = 0isize;
                    let mut end = body;
                    loop {
                        depth += lines[end].matches('{').count() as isize;
                        depth -= lines[end].matches('}').count() as isize;
                        if depth <= 0 || end + 1 >= lines.len() {
                            break;
                        }
                        end += 1;
                    }
                    index = end + 1;
                    continue;
                }
            }
            kept.push_str(lines[index]);
            kept.push('\n');
            index += 1;
        }
        kept
    }

    /// Every `(file, alias)` pair one source declares. Split out from the
    /// workspace walk so the scanner itself has a test over text it can be given.
    fn alias_sites_in_source(file: &str, source: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if !source.contains("alias") {
            return out;
        }
        for (index, line) in source.lines().enumerate() {
            // An attribute counts only when `#[serde(` opens its line, which
            // is how rustfmt writes every real one of them.
            if !line.trim_start().starts_with("#[serde(") {
                continue;
            }
            for alias in alias_sites_in(&attribute_text(source, index)) {
                out.push((file.to_owned(), alias));
            }
        }
        out
    }

    /// The text of the `#[serde(...)]` attribute opening on `line`, which
    /// rustfmt may spread over several lines.
    fn attribute_text(source: &str, line: usize) -> String {
        let mut text = String::new();
        let mut depth = 0usize;
        for line in source.lines().skip(line) {
            text.push_str(line);
            text.push('\n');
            for ch in line.chars() {
                match ch {
                    '(' => depth += 1,
                    ')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            if depth == 0 {
                break;
            }
        }
        text
    }

    /// Every string an `alias` key names in one attribute's text. Whitespace
    /// is collapsed first, so a key rustfmt split across lines is still read.
    /// Requiring a separator before `alias` is what keeps clap's
    /// `visible_alias` and `alias` args out of the read even if one ever sat
    /// inside a `#[serde(` line.
    fn alias_sites_in(attribute: &str) -> Vec<String> {
        const KEY: &str = "alias=\"";
        let flat: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
        let mut out = Vec::new();
        let mut rest = flat.as_str();
        while let Some(pos) = rest.find(KEY) {
            let preceded_by_separator =
                pos == 0 || matches!(rest.as_bytes()[pos - 1], b'[' | b'(' | b',');
            if preceded_by_separator {
                let after = &rest[pos + KEY.len()..];
                match after.find('"') {
                    Some(end) => {
                        out.push(after[..end].to_owned());
                        rest = &after[end..];
                    }
                    None => break,
                }
            } else {
                rest = &rest[pos + 1..];
            }
        }
        out
    }

    /// Every `.rs` file under `dir`, recursively. `target/` and `.git/` never
    /// appear under `crates/`, so a plain walk is enough.
    fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                out.extend(rust_files(&path));
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        out
    }

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::parent)
            .expect("crates/common/xai-tool-types sits three levels under the workspace root")
            .to_path_buf()
    }

    /// The `[model.<id>]` table resolves a both-keys config in
    /// `config_model_override_parse::dedupe_aliases` before serde sees it. That
    /// function's `ALIASES` list and this module's tables describe one set of
    /// keys twice. Lists that can be edited apart is the failure that file's
    /// own comment warns about, so this asserts they name the same pairs. Every
    /// pair in `ALIASES` appears here against `ConfigModelOverride`, and every
    /// pair this module claims for that type appears there.
    #[test]
    fn the_override_dedupe_list_and_these_tables_name_the_same_pairs() {
        let root = workspace_root();
        let parse_file = "crates/codegen/xai-grok-shell/src/agent/config_model_override_parse.rs";
        let source = read_table_file(&root, parse_file);
        let declared = table_pairs_for("ConfigModelOverride");

        let start = source
            .find("const ALIASES: &[(&str, &str)] = &[")
            .expect("config_model_override_parse declares its alias pairs in an ALIASES const");
        let body = &source[start..];
        let body = &body[..body
            .find("];")
            .expect("`ALIASES` is a closed array literal")];
        let there: Vec<(String, String)> = quoted_pairs(body);

        assert!(
            !there.is_empty(),
            "`ALIASES` in {parse_file} names no pair, so nothing here is being cross-checked"
        );
        for (canonical, legacy) in &there {
            assert!(
                declared.iter().any(|(c, l)| c == canonical && l == legacy),
                "{parse_file} dedupes `{canonical}` against `{legacy}`, which no entry in \
                 WIRED or LOCAL declares - the two lists have been edited apart"
            );
        }
        for (canonical, legacy) in &declared {
            assert!(
                there.iter().any(|(c, l)| c == canonical && l == legacy),
                "this module declares `{canonical}`/`{legacy}` for ConfigModelOverride, which \
                 {parse_file}'s `ALIASES` does not dedupe - serde would see a duplicate field \
                 before any fold ran"
            );
        }
    }

    /// `(canonical, alias)` pairs this module claims for a type, across both
    /// tables.
    fn table_pairs_for(ty: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = WIRED
            .iter()
            .filter(|entry| entry.ty == ty)
            .flat_map(|entry| {
                entry
                    .aliases
                    .iter()
                    .map(move |alias| (entry.canonical.to_owned(), (*alias).to_owned()))
            })
            .collect();
        out.extend(
            LOCAL
                .iter()
                .filter(|entry| entry.ty == ty)
                .flat_map(|entry| {
                    entry
                        .aliases
                        .iter()
                        .map(move |alias| (entry.canonical.to_owned(), (*alias).to_owned()))
                }),
        );
        out.sort();
        out
    }

    fn quoted_pairs(body: &str) -> Vec<(String, String)> {
        let words: Vec<String> = body
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|field| (*field).to_owned())
            .collect();
        let mut pairs = Vec::new();
        for pair in words.chunks(2) {
            if pair.len() == 2 {
                pairs.push((pair[0].clone(), pair[1].clone()));
            }
        }
        pairs
    }
}
