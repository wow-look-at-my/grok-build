//! PTY: the route dialog shows a provider's hop chain. A toggle is saved, and the next request takes the new path.
#[allow(unused_imports)]
use super::common::*;
use super::provider_only_config_keeps_typed_prompt::FIRST_PARTY_ENV;
use xai_grok_pager_pty_harness::EnvOp;

const TIMEOUT: Duration = Duration::from_secs(20);
/// The hop adds this header. A request that carries it went through the hop.
const HOP_HEADER: &str = "x-test-hop";

/// One provider behind one optional relay. The relay's route is the mock's own
/// URL, so both paths reach the mock and only the header tells them apart.
fn write_route_config(content: &ContentController) {
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create GROK_HOME");
    let url = content.url();
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[model_providers.internal]\napi_backend = \"chat_completions\"\n\
             api_base_url = \"{url}\"\nenv_key = [\"PROVIDER_AUTH_TOKEN\"]\n\n\
             [[model_providers.internal.hops]]\nid = \"nas\"\nname = \"NAS\"\n\
             kind = \"relay\"\noptional = true\nbase_url = \"{url}\"\n\
             routes = {{ internal = \"{url}\" }}\n\
             extra_headers = {{ {HOP_HEADER} = \"nas\" }}\n\n\
             [model.my-model]\nmodel = \"test-model\"\nmodel_provider = \"internal\"\n"
        ),
    )
    .expect("write config.toml");
}

fn config_text(content: &ContentController) -> String {
    std::fs::read_to_string(content.home().join(".grok").join("config.json")).unwrap_or_default()
}

fn debug_log(content: &ContentController) -> String {
    std::fs::read_to_string(content.home().join("debug.log")).unwrap_or_default()
}

/// The hop header of each chat request, in arrival order.
fn hop_header_per_request(content: &ContentController) -> Vec<Option<String>> {
    content
        .requests()
        .iter()
        .filter(|entry| entry.path.ends_with("/chat/completions"))
        .map(|entry| entry.header(HOP_HEADER).map(str::to_owned))
        .collect()
}

fn run_turn(harness: &mut PtyHarness, content: &ContentController, prompt: &str, answer: &str) {
    content.set_response(answer.to_owned());
    harness
        .inject_keys(format!("{prompt}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(answer, Duration::from_secs(30))
        .unwrap_or_else(|_| {
            panic!(
                "no answer to {prompt:?}\nscreen:\n{}\nrequests: {:?}",
                harness.screen_contents(),
                content
                    .requests()
                    .iter()
                    .map(|entry| &entry.path)
                    .collect::<Vec<_>>()
            )
        });
    harness.update(Duration::from_millis(400));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires a rebuilt pager binary"]
async fn a_hop_toggled_in_the_route_dialog_changes_the_next_request() {
    let content = ContentController::start().await.expect("start content");
    write_route_config(&content);
    let log_path = content.home().join("debug.log");
    let log_path = log_path.to_str().expect("utf-8 log path");
    let mut ops: Vec<EnvOp<'_>> = FIRST_PARTY_ENV.iter().map(|k| EnvOp::remove(k)).collect();
    ops.push(EnvOp::set("PROVIDER_AUTH_TOKEN", "internal-key"));
    ops.push(EnvOp::set("GROK_DEBUG_LOG", log_path));
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &ops,
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    run_turn(
        &mut harness,
        &content,
        "first prompt",
        "ANSWER-THROUGH-THE-HOP",
    );
    assert_eq!(
        hop_header_per_request(&content).last(),
        Some(&Some("nas".to_owned())),
        "the first request must go through the hop"
    );

    harness.inject_keys(b"/hops\r").expect("open the dialog");
    harness
        .wait_for_text("[x] NAS", TIMEOUT)
        .unwrap_or_else(|_| panic!("the dialog lists the hop\n{}", harness.screen_contents()));
    let screen = harness.screen_contents();
    for expected in [
        "Route \u{b7}",
        "relay",
        "internal",
        "provider",
        "Sends to",
        "Last request",
    ] {
        assert!(screen.contains(expected), "no {expected:?} in:\n{screen}");
    }
    // Both links are the mock, and it answers a probe.
    harness
        .wait_for_text(" ms", TIMEOUT)
        .unwrap_or_else(|_| panic!("a link is measured\n{}", harness.screen_contents()));

    harness.inject_keys(b" ").expect("toggle the hop");
    harness
        .wait_for_text("[ ] NAS", TIMEOUT)
        .unwrap_or_else(|_| panic!("the row flips\n{}", harness.screen_contents()));
    harness
        .wait_for_text("skipped", TIMEOUT)
        .unwrap_or_else(|_| panic!("an off hop is not measured\n{}", harness.screen_contents()));
    harness
        .wait_until("config.json holds the toggle", TIMEOUT, |_| {
            config_text(&content).contains("\"enabled\": false")
        })
        .unwrap_or_else(|_| panic!("config.json:\n{}", config_text(&content)));
    // The config watcher publishes the new path. The line names the hops that are on.
    harness
        .wait_until("the route without the hop is published", TIMEOUT, |_| {
            debug_log(&content).lines().any(|line| {
                line.contains("provider routes set")
                    && line.contains("internal at ")
                    && !line.contains("NAS")
            })
        })
        .unwrap_or_else(|_| {
            panic!(
                "the new route was not published\nlog:\n{}",
                debug_log(&content)
                    .lines()
                    .filter(|line| line.contains("route"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });

    harness.inject_keys(keys::ESC).expect("close the dialog");
    harness
        .wait_for_text_absent("Sends to", TIMEOUT)
        .expect("dialog closed");

    run_turn(
        &mut harness,
        &content,
        "second prompt",
        "ANSWER-STRAIGHT-TO-THE-PROVIDER",
    );
    assert_eq!(
        hop_header_per_request(&content).last(),
        Some(&None),
        "the request after the toggle must skip the hop"
    );

    harness.quit().expect("clean quit");
}
