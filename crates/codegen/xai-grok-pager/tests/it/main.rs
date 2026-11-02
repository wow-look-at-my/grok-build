//! All integration tests of this crate, linked as one binary. Select one file with a name filter, e.g. `settings_e2e::`.
//!
//! The tests that set `GROK_HOME` need a process of their own. nextest gives each test one.

mod ci_status_gh;
mod grok_home_paths;
mod mermaid_render_subprocess;
mod plugin_marketplace_remove_locked;
mod plugin_marketplace_remove_official;
mod registered_features_are_documented;
mod selection_model_public_api;
mod settings_e2e;
mod signal_errno_preservation;
