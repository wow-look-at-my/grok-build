//! A session signed in to xAI runs a turn on a `[model_providers.<id>]`
//! gateway model.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{self as acp, Agent as _};
use serde_json::json;
use tempfile::TempDir;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use xai_acp_lib::{
    AcpAgentGatewayReceiver as GatewayReceiver, AcpAgentGatewaySender as GatewaySender,
    LineBufferedRead,
};
use xai_grok_shell::agent::mvp_agent::MvpAgent;
use xai_grok_test_support::mock_server::LogEntry;
use xai_grok_test_support::{MockInferenceServer, MockModelEntry};

const DUPLEX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(60);

const SESSION_TOKEN: &str = "xai-session-token-for-the-gateway-test";
const GATEWAY_KEY: &str = "sk-gateway-test";
const GATEWAY_HOST: &str = "gateway.test";
const GATEWAY_SLUG: &str = "openrouter/deepseek/deepseek-v4.1-flash-1017";
const XAI_MODEL: &str = "test-model";

struct QuietClient;

#[async_trait::async_trait(?Send)]
impl acp::Client for QuietClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        let outcome = args
            .options
            .first()
            .map(|o| {
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    o.option_id.clone(),
                ))
            })
            .unwrap_or(acp::RequestPermissionOutcome::Cancelled);
        Ok(acp::RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(&self, _args: acp::SessionNotification) -> acp::Result<()> {
        Ok(())
    }

    async fn ext_notification(&self, _args: acp::ExtNotification) -> acp::Result<()> {
        Ok(())
    }
}

fn seed_home(grok_home: &std::path::Path, xai_url: &str, gateway_url: &str) {
    std::fs::write(grok_home.join("agent_id"), "test-agent-id").expect("seed agent_id");
    let scope = xai_grok_login::GrokComConfig::default().auth_scope();
    let auth = json!({
        scope: {
            "key": SESSION_TOKEN,
            "auth_mode": "oidc",
            "oidc_issuer": xai_grok_login::xai_oauth2_issuer(),
            "create_time": "2026-01-01T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "user_id": "test-user",
        }
    });
    std::fs::write(grok_home.join("auth.json"), auth.to_string()).expect("write auth.json");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            r#"[endpoints]
cli_chat_proxy_base_url = "{xai_url}"

[model_providers.pazer]
base_url = "{gateway_url}"
api_backend = "chat_completions"
api_key = "{GATEWAY_KEY}"
models_autodetect = true
"#
        ),
    )
    .expect("write config.toml");
}

/// The agent the CLI builds: config read from `GROK_HOME`, then initialize and a cached-token authenticate.
async fn connect() -> acp::ClientSideConnection {
    let agent_config =
        xai_grok_shell::config::load_agent_config_disk_only().expect("load config.toml");
    let auth_manager = Arc::new(agent_config.create_auth_manager());
    let (gw_tx, gw_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent = MvpAgent::new(
        GatewaySender::new(gw_tx),
        &agent_config,
        auth_manager,
        None,
        None,
    )
    .expect("valid config");

    let (c2a_a, c2a_b) = tokio::io::duplex(DUPLEX_BUFFER_BYTES);
    let (a2c_a, a2c_b) = tokio::io::duplex(DUPLEX_BUFFER_BYTES);

    let agent_incoming = LineBufferedRead::spawn_local(c2a_b.compat());
    let (agent_conn, agent_io) =
        acp::AgentSideConnection::new(agent, a2c_a.compat_write(), agent_incoming, |fut| {
            tokio::task::spawn_local(fut);
        });
    tokio::task::spawn_local(
        GatewayReceiver::new(gw_rx, agent_conn)
            .with_on_meta(xai_grok_otel::span_from_meta_traceparent)
            .run(),
    );
    tokio::task::spawn_local(agent_io);

    let client_incoming = LineBufferedRead::spawn_local(a2c_b.compat());
    let (client_conn, client_io) =
        acp::ClientSideConnection::new(QuietClient, c2a_a.compat_write(), client_incoming, |fut| {
            tokio::task::spawn_local(fut);
        });
    tokio::task::spawn_local(client_io);

    tokio::time::timeout(
        RPC_TIMEOUT,
        client_conn.initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                .client_capabilities(
                    acp::ClientCapabilities::new()
                        .fs(acp::FileSystemCapabilities::new())
                        .terminal(false),
                )
                .meta(
                    json!({
                        "startupHints": { "nonInteractive": true },
                        "clientType": "provider-gateway-credentials",
                        "clientVersion": "0.0-test",
                    })
                    .as_object()
                    .cloned(),
                ),
        ),
    )
    .await
    .expect("initialize timed out")
    .expect("initialize failed");

    tokio::time::timeout(
        RPC_TIMEOUT,
        client_conn.authenticate(
            acp::AuthenticateRequest::new(acp::AuthMethodId::new("cached_token"))
                .meta(json!({ "headless": true }).as_object().cloned()),
        ),
    )
    .await
    .expect("authenticate timed out")
    .expect("the seeded xAI session must authenticate");

    client_conn
}

/// A new session waits for provider discovery, so the gateway's models are listed by then.
async fn run_turn(conn: &acp::ClientSideConnection, cwd: &std::path::Path, model_id: &str) {
    let session = tokio::time::timeout(
        RPC_TIMEOUT,
        conn.new_session(
            acp::NewSessionRequest::new(cwd.to_path_buf())
                .meta(json!({ "modelId": model_id }).as_object().cloned()),
        ),
    )
    .await
    .expect("session/new timed out")
    .expect("session/new failed");

    tokio::time::timeout(
        RPC_TIMEOUT,
        conn.prompt(acp::PromptRequest::new(
            session.session_id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(
                "say hi".to_string(),
            ))],
        )),
    )
    .await
    .expect("prompt timed out")
    .unwrap_or_else(|e| panic!("the turn on {model_id} must succeed: {e:?}"));
}

fn inference_requests(server: &MockInferenceServer) -> Vec<LogEntry> {
    server
        .requests()
        .into_iter()
        .filter(|e| e.method == "POST" && e.path.contains("chat/completions"))
        .collect()
}

fn headers_carrying_session(entry: &LogEntry) -> Vec<String> {
    entry
        .headers
        .iter()
        .filter(|(_, v)| v.contains(SESSION_TOKEN))
        .map(|(k, _)| k.clone())
        .collect()
}

#[test]
fn gateway_model_gets_the_gateway_key_and_never_the_xai_session() {
    xai_grok_extra_ca::ensure_default_crypto_provider();

    let mock_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("mock runtime");
    let gateway = mock_rt
        .block_on(MockInferenceServer::start_with_models(vec![
            MockModelEntry::new(GATEWAY_SLUG),
        ]))
        .expect("gateway mock");
    let xai = mock_rt
        .block_on(MockInferenceServer::start_with_models(vec![
            MockModelEntry::new(XAI_MODEL),
        ]))
        .expect("xai mock");

    let gateway_port = gateway
        .origin()
        .rsplit(':')
        .next()
        .expect("gateway origin has a port")
        .to_owned();
    let gateway_url = format!("http://{GATEWAY_HOST}:{gateway_port}/v1");

    let grok_home = TempDir::new().expect("grok home");
    let workdir = TempDir::new().expect("workdir");
    seed_home(grok_home.path(), &xai.url(), &gateway_url);

    // SAFETY: the only other live threads are the mock runtime's HTTP workers,
    // which never read the process environment.
    unsafe {
        xai_grok_test_support::isolate_grok_env(grok_home.path());
        std::env::set_var("GROK_CLI_CHAT_PROXY_BASE_URL", xai.url());
        std::env::set_var("GROK_XAI_API_BASE_URL", xai.url());
        std::env::set_var("GROK_MODELS_BASE_URL", xai.url());
        // `gateway.test` resolves nowhere. The proxy sends its requests to the gateway mock.
        std::env::set_var("HTTP_PROXY", gateway.origin());
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
        std::env::remove_var("GROK_CODE_XAI_API_KEY");
    }
    xai_grok_net::register_session_token(SESSION_TOKEN);

    let agent_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("agent runtime");
    let local = tokio::task::LocalSet::new();
    agent_rt.block_on(local.run_until(async {
        let conn = connect().await;

        run_turn(&conn, workdir.path(), &format!("pazer/{GATEWAY_SLUG}")).await;
        let sent = inference_requests(&gateway);
        assert!(
            !sent.is_empty(),
            "the turn must reach the gateway; it got: {}",
            gateway.request_log_summary()
        );
        for entry in &sent {
            assert!(
                entry
                    .header("host")
                    .is_some_and(|h| h.starts_with(GATEWAY_HOST)),
                "the request must be addressed to the third-party host: {:?}",
                entry.header("host")
            );
            assert_eq!(
                entry.header("authorization"),
                Some(format!("Bearer {GATEWAY_KEY}").as_str()),
                "the gateway must get its own key as the bearer"
            );
            assert_eq!(
                entry.header("x-bf-vk"),
                Some(GATEWAY_KEY),
                "the gateway reads its virtual key from x-bf-vk"
            );
        }
        for entry in gateway.requests() {
            let leaked = headers_carrying_session(&entry);
            assert!(
                leaked.is_empty(),
                "{} {} carried the xAI session token in {leaked:?}",
                entry.method,
                entry.path
            );
        }
        assert!(
            inference_requests(&xai).is_empty(),
            "a gateway model must not send its prompt to the xAI host"
        );

        // Loopback counts as an xAI host in this build.
        run_turn(&conn, workdir.path(), XAI_MODEL).await;
        let sent = inference_requests(&xai);
        assert!(
            !sent.is_empty(),
            "the turn must reach the xAI host; it got: {}",
            xai.request_log_summary()
        );
        for entry in &sent {
            assert_eq!(
                entry.header("authorization"),
                Some(format!("Bearer {SESSION_TOKEN}").as_str()),
                "an xAI host must get the session token"
            );
        }
    }));
}
