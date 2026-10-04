//! End-to-end guard for `auth_provider_command`: a configured external auth provider must mint the session credential.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::Utc;
use xai_grok_login::{AuthMode, GrokAuth, GrokComConfig, try_ensure_fresh_auth};

const SEED_TOKEN: &str = "stale-token-that-must-be-replaced";

/// `grok_home()` memoizes into a `OnceLock`, so every phase below shares this
/// directory.
fn use_temp_grok_home(dir: &Path) {
    // SAFETY: single-threaded test entry, before any thread that reads the
    // environment is spawned.
    unsafe {
        std::env::set_var("GROK_HOME", dir);
    }
}

/// Seed an expired credential so `auth()` takes the refresh path; a cold home returns `NotLoggedIn` without ever consulting the provider.
fn seed_expired_credential(home: &Path, scope: &str) {
    let expired = GrokAuth {
        key: SEED_TOKEN.to_owned(),
        auth_mode: AuthMode::External,
        expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
        ..GrokAuth::default()
    };
    let store: BTreeMap<String, GrokAuth> = [(scope.to_owned(), expired)].into_iter().collect();
    std::fs::write(
        home.join("auth.json"),
        serde_json::to_string(&store).expect("serialize auth store"),
    )
    .expect("write auth.json");
}

/// Run one provider command through the real auth path and return the token.
async fn mint_with_provider(home: &Path, command: &str) -> String {
    let config = GrokComConfig {
        auth_provider_command: Some(command.to_owned()),
        ..GrokComConfig::default()
    };
    seed_expired_credential(home, &config.auth_scope());

    let auth = try_ensure_fresh_auth(
        &config,
        xai_grok_shell::agent::config::CLI_CHAT_PROXY_BASE_URL_DEFAULT.to_string(),
    )
    .await
    .unwrap_or_else(|| {
        panic!("auth_provider_command `{command}` was configured but no credential was minted")
    });
    assert_eq!(
        auth.auth_mode,
        AuthMode::External,
        "credential must come from the provider, not a cached or built-in path"
    );
    assert_ne!(
        auth.key, SEED_TOKEN,
        "the expired seed must have been replaced by the provider's output"
    );
    auth.key
}

#[tokio::test]
async fn auth_provider_command_mints_the_session_credential() {
    let home = tempfile::tempdir().expect("tempdir");
    use_temp_grok_home(home.path());

    // `echo <token>` is valid in both `sh -c` and `cmd /C`, so this phase needs no external binary and runs identically on every platform
    let token = mint_with_provider(home.path(), "echo grok-ext-token").await;
    assert_eq!(token, "grok-ext-token");

    // Windows only: an absolute native path, the form an operator writes in
    // config.toml, and the exact shape a POSIX shell mangles Run.
    #[cfg(windows)]
    {
        let token = mint_with_provider(home.path(), r"C:\Windows\System32\whoami.exe").await;
        assert!(
            !token.trim().is_empty(),
            "a native Windows path must reach the provider intact"
        );
    }
}
