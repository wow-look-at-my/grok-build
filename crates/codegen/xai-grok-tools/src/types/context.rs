use crate::implementations::grok_build::read_file::MAX_LINES_READ;
use std::collections::HashMap;

/// Client-configurable truncation settings. Scalar fields are optional — `None` means "use the tool's built-in default".
/// There is deliberately no per-line cap: clipping long lines silently corrupts single-line files (minified JSON, data
/// dumps) with no way for the model to recover the clipped bytes.
#[derive(Debug, Clone, Default)]
pub struct TruncationConfig {
    /// Max total output bytes for any tool. Default: 40KB.
    pub default_max_output_bytes: Option<usize>,
    /// Per-tool overrides keyed by canonical tool name.
    pub per_tool_max_output_bytes: HashMap<String, usize>,
    /// Max lines to read (read_file). Default: 1000.
    pub max_lines_read: Option<usize>,
    /// Inline cap for MCP tool results only (bytes). Consulted by the MCP truncation path (`mcp_max_output_bytes_for`) between the per-tool map and
    /// `default_max_output_bytes`. Deliberately separate from `default_max_output_bytes` so an MCP-specific override (e.g. a repo's `[mcp]
    /// max_output_bytes`) never changes non-MCP readers like the opencode bash cap.
    pub mcp_max_output_bytes: Option<usize>,
    /// Live ceiling derived from the model's remaining context-window budget (see `SessionActor::reseed_context_budget_output_cap` in xai-grok-shell), re-resolved before each tool-dispatch step.
    pub context_budget_max_output_bytes: Option<usize>,
    pub whole_read: WholeReadPolicy,
}

/// Which files `read_file` returns whole under the token cap; a bit that is off makes its class a regular file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct WholeReadPolicy {
    pub skill_markdown: bool,
    pub instruction_files: bool,
}

impl Default for WholeReadPolicy {
    fn default() -> WholeReadPolicy {
        WholeReadPolicy {
            skill_markdown: true,
            instruction_files: true,
        }
    }
}

impl TruncationConfig {
    /// Resolved max lines per `read_file` window.
    pub fn max_lines_read(&self) -> usize {
        self.max_lines_read.unwrap_or(MAX_LINES_READ)
    }

    /// Resolve the max output bytes for a specific tool.
    ///
    /// Precedence: per-tool override > default override > built-in fallback,
    /// then clamped to [`Self::context_budget_max_output_bytes`]. This happens
    /// when set (see its docs) so the live budget always wins over a static
    /// config value.
    pub fn max_output_bytes_for(&self, tool_name: &str, builtin_default: usize) -> usize {
        let resolved = if let Some(&per_tool) = self.per_tool_max_output_bytes.get(tool_name) {
            per_tool
        } else {
            self.default_max_output_bytes.unwrap_or(builtin_default)
        };
        self.clamp_to_context_budget(resolved)
    }

    /// Clamp `resolved` to [`Self::context_budget_max_output_bytes`] when set.
    fn clamp_to_context_budget(&self, resolved: usize) -> usize {
        match self.context_budget_max_output_bytes {
            Some(budget) => resolved.min(budget),
            None => resolved,
        }
    }

    /// Resolve the max output bytes for an **MCP** payload. Precedence: per-tool override > MCP-specific override (`mcp_max_output_bytes`) >
    /// default override > built-in fallback. Only the MCP truncation path (`util::mcp_truncate`) should call this; non-MCP tools keep using
    /// [`Self::max_output_bytes_for`] so that an MCP-specific override never bleeds into their caps.
    pub fn mcp_max_output_bytes_for(&self, tool_name: &str, builtin_default: usize) -> usize {
        let resolved = if let Some(&per_tool) = self.per_tool_max_output_bytes.get(tool_name) {
            per_tool
        } else {
            self.mcp_max_output_bytes
                .or(self.default_max_output_bytes)
                .unwrap_or(builtin_default)
        };
        self.clamp_to_context_budget(resolved)
    }

    /// Replace template placeholders in a tool description with current config values. `{max_lines_read}` — from `max_lines_read` (default 1000)
    /// `{max_chars_per_line}` — fixed display value for opencode-compat descriptions only; the opencode `read` tool clips at its own hardcoded
    /// `MAX_LINE_LENGTH` (2000), independent of this config. grok_build `read_file` never clips lines.
    pub fn interpolate_description(
        &self,
        description: &str,
        tool_name: &str,
        builtin_output_default: usize,
    ) -> String {
        description
            .replace("{max_lines_read}", &self.max_lines_read().to_string())
            .replace("{max_chars_per_line}", "2000")
            .replace(
                "{max_output_bytes}",
                &self
                    .max_output_bytes_for(tool_name, builtin_output_default)
                    .to_string(),
            )
    }

    /// Resolve placeholders in each schema property description.
    pub fn apply_to_schema(
        &self,
        schema: &mut serde_json::Value,
        tool_name: &str,
        builtin_output_default: usize,
    ) {
        let Some(properties) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) else {
            return;
        };
        for property in properties.values_mut() {
            let Some(object) = property.as_object_mut() else {
                continue;
            };
            let Some(description) = object.get("description").and_then(|d| d.as_str()) else {
                continue;
            };
            let resolved =
                self.interpolate_description(description, tool_name, builtin_output_default);
            object.insert("description".to_string(), serde_json::json!(resolved));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_lines_read_default_and_override() {
        assert_eq!(TruncationConfig::default().max_lines_read(), MAX_LINES_READ);
        let cfg = TruncationConfig {
            max_lines_read: Some(50),
            ..Default::default()
        };
        assert_eq!(cfg.max_lines_read(), 50);
    }

    #[test]
    fn apply_to_schema_resolves_property_placeholders() {
        let cfg = TruncationConfig {
            max_lines_read: Some(50),
            ..Default::default()
        };
        let mut schema = serde_json::json!({
            "properties": {
                "limit": {"type": "integer", "description": "Up to {max_lines_read} lines."},
                "task_ids": {"type": "array", "description": "Task IDs."},
            }
        });
        cfg.apply_to_schema(&mut schema, "read_file", 40_000);

        assert_eq!(
            schema
                .pointer("/properties/limit/description")
                .and_then(|v| v.as_str()),
            Some("Up to 50 lines.")
        );
        assert_eq!(
            schema
                .pointer("/properties/task_ids/description")
                .and_then(|v| v.as_str()),
            Some("Task IDs.")
        );
        assert!(
            schema.pointer("/properties/limit/maximum").is_none(),
            "{schema}"
        );
    }

    #[test]
    fn apply_to_schema_tolerates_schemas_without_properties() {
        let mut schema = serde_json::json!({"type": "object"});
        TruncationConfig::default().apply_to_schema(&mut schema, "get_task_output", 40_000);
        assert_eq!(schema, serde_json::json!({"type": "object"}));
    }

    #[test]
    fn mcp_max_output_bytes_for_lookup_order() {
        // per-tool > mcp-specific > default > builtin
        let cfg = TruncationConfig {
            default_max_output_bytes: Some(1_000),
            per_tool_max_output_bytes: HashMap::from([("use_tool".to_string(), 111)]),
            mcp_max_output_bytes: Some(500),
            ..Default::default()
        };
        assert_eq!(cfg.mcp_max_output_bytes_for("use_tool", 9_999), 111);
        assert_eq!(cfg.mcp_max_output_bytes_for("CallMcpTool", 9_999), 500);

        let no_mcp = TruncationConfig {
            default_max_output_bytes: Some(1_000),
            ..Default::default()
        };
        assert_eq!(no_mcp.mcp_max_output_bytes_for("use_tool", 9_999), 1_000);
        assert_eq!(
            TruncationConfig::default().mcp_max_output_bytes_for("use_tool", 9_999),
            9_999
        );
    }

    #[test]
    fn context_budget_clamps_a_static_default_and_a_per_tool_override() {
        let cfg = TruncationConfig {
            default_max_output_bytes: Some(40_000),
            per_tool_max_output_bytes: HashMap::from([("run_terminal_cmd".to_string(), 500_000)]),
            context_budget_max_output_bytes: Some(1_000),
            ..Default::default()
        };
        // Static default is already above the budget: clamped down.
        assert_eq!(cfg.max_output_bytes_for("grep", 20_000), 1_000);
        // A generous per-tool override does not escape the live budget either.
        assert_eq!(cfg.max_output_bytes_for("run_terminal_cmd", 20_000), 1_000);
        // MCP resolution goes through the same clamp.
        assert_eq!(cfg.mcp_max_output_bytes_for("use_tool", 20_000), 1_000);
    }

    #[test]
    fn context_budget_never_widens_a_smaller_static_cap() {
        // A generous remaining budget must not override a deliberately small
        // static/per-tool cap — the budget is a ceiling, not a floor.
        let cfg = TruncationConfig {
            default_max_output_bytes: Some(2_000),
            context_budget_max_output_bytes: Some(1_000_000),
            ..Default::default()
        };
        assert_eq!(cfg.max_output_bytes_for("grep", 20_000), 2_000);
    }

    #[test]
    fn context_budget_absent_is_a_no_op() {
        let cfg = TruncationConfig {
            default_max_output_bytes: Some(40_000),
            ..Default::default()
        };
        assert_eq!(cfg.max_output_bytes_for("grep", 20_000), 40_000);
    }

    #[test]
    fn mcp_override_does_not_bleed_into_non_mcp_lookup() {
        // Regression: the MCP-specific cap must not change what non-MCP
        // readers (e.g. opencode bash via `max_output_bytes_for("bash", ..)`)
        // resolve.
        let cfg = TruncationConfig {
            mcp_max_output_bytes: Some(123),
            ..Default::default()
        };
        assert_eq!(cfg.max_output_bytes_for("bash", 20_000), 20_000);
        assert_eq!(cfg.mcp_max_output_bytes_for("use_tool", 20_000), 123);
    }
}
