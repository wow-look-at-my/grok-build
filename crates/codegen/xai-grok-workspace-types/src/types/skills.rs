//! Discovery shapes for skills carried by `OpsChunk::Skills` and `WorkspaceEvent::SkillsChanged`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInfo {
    /// Stable identifier (also the slash-command name).
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub display_name: String,
    /// Short human-readable description.
    #[serde(default)]
    pub description: String,
    /// Path to the skill definition (typically `SKILL.md`) as a string.
    #[serde(default)]
    pub path: String,
    /// Source bucket: `"global"`, `"workspace"`, `"server"`, `"bundled"`, etc.
    #[serde(default)]
    pub source: String,
}
