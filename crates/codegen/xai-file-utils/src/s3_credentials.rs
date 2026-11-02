//! Where S3 credentials come from: explicit content or file, else.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::s3_sigv4::Credentials;

/// Parse credential content (JSON or INI format).
pub(crate) fn parse_credentials_content(content: &str) -> anyhow::Result<Credentials> {
    #[derive(serde::Deserialize)]
    struct JsonCreds {
        aws_access_key_id: String,
        aws_secret_access_key: String,
        #[serde(default)]
        aws_session_token: Option<String>,
    }

    if let Ok(parsed) = serde_json::from_str::<JsonCreds>(content) {
        return Ok(Credentials {
            access_key_id: parsed.aws_access_key_id,
            secret_access_key: parsed.aws_secret_access_key,
            session_token: parsed.aws_session_token,
        });
    }

    let mut keys = KeyLines::default();
    for line in content.lines() {
        keys.read(line);
    }
    keys.into_credentials().ok_or_else(|| {
        anyhow::anyhow!(
            "AWS credentials are neither valid JSON \
             nor contain aws_access_key_id and aws_secret_access_key"
        )
    })
}

#[derive(Default)]
struct KeyLines {
    key_id: Option<String>,
    secret: Option<String>,
    token: Option<String>,
}

impl KeyLines {
    fn read(&mut self, line: &str) {
        let strip_comment = |v: &str| {
            v.split_once('#')
                .map_or(v, |(before, _)| before)
                .trim()
                .to_owned()
        };
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "aws_access_key_id" => self.key_id = Some(strip_comment(v)),
                "aws_secret_access_key" => self.secret = Some(strip_comment(v)),
                "aws_session_token" => self.token = Some(strip_comment(v)),
                _ => {}
            }
        }
    }

    fn into_credentials(self) -> Option<Credentials> {
        match (self.key_id, self.secret) {
            (Some(k), Some(s)) if !k.is_empty() && !s.is_empty() => Some(Credentials {
                access_key_id: k,
                secret_access_key: s,
                session_token: self.token.filter(|t| !t.is_empty()),
            }),
            _ => None,
        }
    }
}

/// The static keys of `profile` in an INI file. The config file names a profile
/// `[profile name]`, except `[default]`; the credentials file names it `[name]`.
fn profile_keys(text: &str, profile: &str, config_file: bool) -> Option<Credentials> {
    let wanted = if config_file && profile != "default" {
        format!("profile {profile}")
    } else {
        profile.to_owned()
    };
    let mut in_section = false;
    let mut keys = KeyLines::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.split_whitespace().collect::<Vec<_>>().join(" ") == wanted;
            continue;
        }
        if in_section {
            keys.read(line);
        }
    }
    keys.into_credentials()
}

async fn read_optional(path: &Path) -> anyhow::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

/// Credentials from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`,
/// then from the profile `AWS_PROFILE` (or `default`) of the shared credentials file,
/// then of the shared config file.
pub(crate) async fn default_chain(
    env: impl Fn(&str) -> Option<String>,
    home: Option<PathBuf>,
) -> anyhow::Result<Credentials> {
    let var = |name: &str| env(name).filter(|v| !v.is_empty());
    match (var("AWS_ACCESS_KEY_ID"), var("AWS_SECRET_ACCESS_KEY")) {
        (Some(access_key_id), Some(secret_access_key)) => {
            return Ok(Credentials {
                access_key_id,
                secret_access_key,
                session_token: var("AWS_SESSION_TOKEN"),
            });
        }
        (Some(_), None) => {
            anyhow::bail!("AWS_ACCESS_KEY_ID is set but AWS_SECRET_ACCESS_KEY is not")
        }
        (None, Some(_)) => {
            anyhow::bail!("AWS_SECRET_ACCESS_KEY is set but AWS_ACCESS_KEY_ID is not")
        }
        (None, None) => {}
    }

    let profile = var("AWS_PROFILE").unwrap_or_else(|| "default".to_owned());
    let aws_dir = home.map(|h| h.join(".aws"));
    let credentials_path = var("AWS_SHARED_CREDENTIALS_FILE")
        .map(PathBuf::from)
        .or_else(|| aws_dir.as_ref().map(|d| d.join("credentials")));
    let config_path = var("AWS_CONFIG_FILE")
        .map(PathBuf::from)
        .or_else(|| aws_dir.as_ref().map(|d| d.join("config")));

    for (path, config_file) in [(&credentials_path, false), (&config_path, true)] {
        let Some(path) = path else { continue };
        if let Some(text) = read_optional(path).await?
            && let Some(creds) = profile_keys(&text, &profile, config_file)
        {
            return Ok(creds);
        }
    }

    let shown = |p: &Option<PathBuf>| {
        p.as_ref().map_or_else(
            || "<no home directory>".to_owned(),
            |p| p.display().to_string(),
        )
    };
    anyhow::bail!(
        "no S3 credentials found. Tried: trace upload credentials content and file (not set), \
         env AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY (not set), \
         static keys of profile `{profile}` in {} and {}. \
         SSO, credential_process and instance metadata are not supported",
        shown(&credentials_path),
        shown(&config_path)
    )
}

/// Explicit content wins, then an explicit file, then [`default_chain`].
pub(crate) async fn resolve(
    credentials_content: Option<&str>,
    credentials_file: Option<&str>,
) -> anyhow::Result<Credentials> {
    match (credentials_content, credentials_file) {
        (Some(inline), _) => parse_credentials_content(inline),
        (None, Some(path)) => {
            let content = tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("Failed to read AWS credentials file: {path}"))?;
            parse_credentials_content(&content)
        }
        (None, None) => default_chain(|name| std::env::var(name).ok(), xai_dirs::home_dir()).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn parses_json_and_ini_content() {
        let json = parse_credentials_content(
            r#"{"aws_access_key_id":"A","aws_secret_access_key":"S","aws_session_token":"T"}"#,
        )
        .unwrap();
        assert_eq!(json.access_key_id, "A");
        assert_eq!(json.session_token.as_deref(), Some("T"));

        let ini = parse_credentials_content(
            "[default]\naws_access_key_id = A # note\naws_secret_access_key=S\n",
        )
        .unwrap();
        assert_eq!(ini.access_key_id, "A");
        assert_eq!(ini.secret_access_key, "S");
        assert!(ini.session_token.is_none());

        assert!(parse_credentials_content("nothing here").is_err());
    }

    #[tokio::test]
    async fn env_vars_come_first() {
        let creds = default_chain(
            env_of(&[
                ("AWS_ACCESS_KEY_ID", "EK"),
                ("AWS_SECRET_ACCESS_KEY", "ES"),
                ("AWS_SESSION_TOKEN", "ET"),
            ]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(creds.access_key_id, "EK");
        assert_eq!(creds.session_token.as_deref(), Some("ET"));
    }

    #[tokio::test]
    async fn half_set_env_is_an_error() {
        let err = default_chain(env_of(&[("AWS_ACCESS_KEY_ID", "EK")]), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("AWS_SECRET_ACCESS_KEY"), "{err}");
    }

    #[tokio::test]
    async fn reads_the_selected_profile_of_the_credentials_file() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".aws")).unwrap();
        std::fs::write(
            home.path().join(".aws/credentials"),
            "[default]\naws_access_key_id=DK\naws_secret_access_key=DS\n\
             ; comment\n[work]\naws_access_key_id = WK\naws_secret_access_key = WS\n",
        )
        .unwrap();
        let default = default_chain(env_of(&[]), Some(home.path().to_owned()))
            .await
            .unwrap();
        assert_eq!(default.access_key_id, "DK");
        let work = default_chain(
            env_of(&[("AWS_PROFILE", "work")]),
            Some(home.path().to_owned()),
        )
        .await
        .unwrap();
        assert_eq!(work.access_key_id, "WK");
    }

    #[tokio::test]
    async fn falls_back_to_the_config_file_profile() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".aws")).unwrap();
        std::fs::write(
            home.path().join(".aws/config"),
            "[profile work]\nregion = us-west-2\naws_access_key_id=CK\naws_secret_access_key=CS\n",
        )
        .unwrap();
        let creds = default_chain(
            env_of(&[("AWS_PROFILE", "work")]),
            Some(home.path().to_owned()),
        )
        .await
        .unwrap();
        assert_eq!(creds.access_key_id, "CK");
    }

    #[tokio::test]
    async fn shared_credentials_file_env_overrides_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("creds");
        std::fs::write(
            &path,
            "[default]\naws_access_key_id=FK\naws_secret_access_key=FS\n",
        )
        .unwrap();
        let creds = default_chain(
            env_of(&[("AWS_SHARED_CREDENTIALS_FILE", path.to_str().unwrap())]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(creds.access_key_id, "FK");
    }

    #[tokio::test]
    async fn no_source_names_every_source_tried() {
        let home = tempfile::tempdir().unwrap();
        let err = default_chain(env_of(&[]), Some(home.path().to_owned()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("AWS_ACCESS_KEY_ID"), "{err}");
        assert!(err.contains(".aws/credentials"), "{err}");
        assert!(err.contains(".aws/config"), "{err}");
        assert!(err.contains("profile `default`"), "{err}");
    }

    #[tokio::test]
    async fn sso_only_profile_is_not_credentials() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".aws")).unwrap();
        std::fs::write(
            home.path().join(".aws/config"),
            "[default]\nsso_start_url = https://x\nsso_role_name = r\n",
        )
        .unwrap();
        assert!(
            default_chain(env_of(&[]), Some(home.path().to_owned()))
                .await
                .is_err()
        );
    }
}
