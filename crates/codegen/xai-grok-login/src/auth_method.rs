//! First-party API-key environment primitives.

use std::sync::RwLock;

/// Env var that, when set, advertises `xai.api_key` as a viable auth method.
pub const XAI_API_KEY_ENV_VAR: &str = "XAI_API_KEY";

/// Legacy env var name.
pub const LEGACY_XAI_API_KEY_ENV_VAR: &str = "GROK_CODE_XAI_API_KEY";

/// Runtime-loaded keys live here instead of the process env: `set_var` races C `getenv` on other threads (DNS, libgit2).
static RUNTIME_API_KEY: RwLock<Option<String>> = RwLock::new(None);

pub fn set_runtime_xai_api_key(key: &str) {
    *RUNTIME_API_KEY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(key.to_owned());
}

/// Falls back to the process env; an inherited key can only be removed by the shell that exported it.
pub fn clear_runtime_xai_api_key() {
    *RUNTIME_API_KEY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// `std::env::var` that also sees the runtime key when `name` is `XAI_API_KEY`.
pub fn read_env_var(name: &str) -> Result<String, std::env::VarError> {
    if name == XAI_API_KEY_ENV_VAR
        && let Some(key) = RUNTIME_API_KEY
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    {
        return Ok(key);
    }
    std::env::var(name)
}

/// Read the API key from the runtime override or the environment.
pub fn read_xai_api_key_env() -> Result<String, std::env::VarError> {
    read_env_var(XAI_API_KEY_ENV_VAR).or_else(|_| std::env::var(LEGACY_XAI_API_KEY_ENV_VAR))
}

/// Returns `true` if either `XAI_API_KEY` or `GROK_CODE_XAI_API_KEY` is set.
pub fn has_xai_api_key_env() -> bool {
    read_xai_api_key_env().is_ok()
}
