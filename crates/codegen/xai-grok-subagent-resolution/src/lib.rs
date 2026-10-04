//! Extracts the pure-logic "resolution" phase of subagent spawning from `xai-grok-shell` into a reusable library.

#![deny(clippy::indexing_slicing)]

pub mod config;
pub mod context;
pub mod definition;
pub mod overrides;
pub mod resume;
pub mod types;

pub use config::{PersonaIOField, SubagentPersona, SubagentRole};
pub use definition::{
    DefinitionResolutionContext, DefinitionValidationContext, HarnessToolsetContext,
    apply_child_tool_policy, apply_definition_runtime_defaults, apply_harness_toolset,
    available_agent_names, discover_agent_definition, gate_agent_definition,
    render_subagent_initial_user_message, render_subagent_system_prompt, resolve_agent_definition,
    resolve_runtime_config, select_role, subagent_harness_flavor_is_representable,
    validate_agent_name,
};
pub use overrides::{intersect_capability_modes, resolve_effective_overrides};
pub use resume::{ResumeValidationError, validate_resume_identity};
pub use types::{ContextSource, EffectiveRuntimeConfig, ResolutionError, ResumeSourceData};
pub use xai_grok_agent::config::AgentDefinition;
pub use xai_grok_agent::prompt::paths::PathsConfig;
