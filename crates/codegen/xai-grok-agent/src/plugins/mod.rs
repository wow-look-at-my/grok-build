//! Plugin system: discover, load, and manage plugins (including compat layouts).

pub mod discovery;
pub mod git_install;
pub mod hooks_adapter;
pub mod install_registry;
pub mod local_refresh;
pub mod manifest;
pub mod marketplace;
pub mod registry;
pub mod trust;

pub use discovery::{
    DiscoveredPlugin, PluginOrigin, PluginScope, discover_plugins, project_plugin_dirs,
    project_plugin_dirs_in,
};
pub use hooks_adapter::parse_plugin_hooks;
pub use install_registry::InstallRegistry;
pub use manifest::PluginManifest;
pub use registry::{LoadedPlugin, PluginRegistry, SharedPluginRegistryHandle};
pub use trust::TrustStore;
