// A retry shows its whole reason, wrapped under the "Retrying" header.

#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::InferenceRequestMatcher;
use xai_grok_pager_pty_harness::content::InferenceEndpoint;

const END_MARKER: &str = "ENDMARK";
const NARROW_COLS: u16 = 80;

/// The upstream error text. It is longer than a few rows at many columns
/// and shorter than the cap the client puts on an error body.
fn long_reason() -> String {
    let reason = "the upstream inference pool for this model is at capacity and refused the \
                  request before it reached a worker; every replica in the region reported a \
                  full queue, so the gateway answered with a temporary failure and asked the \
                  client to try again after a short wait ENDMARK";
    assert!(reason.chars().count() > 200 && reason.ends_with(END_MARKER));
    reason.to_string()
}

fn failure(reason: &str) -> ScriptedResponse {
    let mut response = ScriptedResponse::json(
        503,
        json!({ "error": { "message": reason, "type": "server_error" } }),
    );
    // Long enough to read the screen before the retry goes out.
    response
        .headers
        .push(("retry-after".to_owned(), "8".to_owned()));
    response
}

fn strip_ws(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn retry_reason_wraps_in_full() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} done."));
    let reason = long_reason();
    // One failure per backend. The retry falls through to the default reply.
    for endpoint in [
        InferenceEndpoint::Responses,
        InferenceEndpoint::ChatCompletions,
    ] {
        let _ = content.expect_response(
            format!("retryable failure ({endpoint:?})"),
            InferenceRequestMatcher::foreground(endpoint),
            failure(&reason),
        );
    }

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, NARROW_COLS, &content)
        .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");

    harness
        .wait_for_text("Retrying", Duration::from_secs(30))
        .expect("retry header");
    // The reason rows arrive in the same frame as the header. Give a slow
    // frame a moment, then judge the screen as it is.
    let _ = harness.wait_until("the end marker", Duration::from_secs(3), |h| {
        h.contains_text(END_MARKER)
    });

    let screen = harness.screen_contents();
    println!("--- screen while retrying ({NARROW_COLS} cols) ---\n{screen}\n--- end screen ---");

    let rows: Vec<&str> = screen.lines().collect();
    let header = rows
        .iter()
        .position(|row| row.contains("Retrying"))
        .unwrap_or_else(|| panic!("the retry header left the screen\nscreen:\n{screen}"));
    assert!(
        rows[header].contains("(1/"),
        "the header must name the attempt\nscreen:\n{screen}"
    );
    let below: String = rows[header + 1..]
        .iter()
        .take_while(|row| !row.trim_start().starts_with('╭'))
        .copied()
        .collect();
    let below = strip_ws(&below);
    assert!(
        below.contains(END_MARKER),
        "the end of the retry reason must be on screen\nscreen:\n{screen}"
    );
    assert!(
        below.contains(&strip_ws(&reason)),
        "the whole retry reason must be on screen under the header\nscreen:\n{screen}"
    );
    let reason_rows = rows[header + 1..]
        .iter()
        .filter(|row| !row.trim().is_empty())
        .take_while(|row| !row.trim_start().starts_with('╭'))
        .count();
    assert!(
        reason_rows >= 3,
        "a reason this long must wrap over several rows at {NARROW_COLS} columns\nscreen:\n{screen}"
    );

    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("the retried request answers");
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
