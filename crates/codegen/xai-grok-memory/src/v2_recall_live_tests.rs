//! Recall against a real embedding model. Each test is ignored: it needs a served model.

use std::collections::HashSet;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::embedding::{ApiEmbeddingProvider, EmbeddingProvider};
use crate::v2::ensure_scope_initialized;

#[derive(Debug, Default)]
struct NoBilling;

impl xai_grok_sampler::UsageSink for NoBilling {
    fn record_call(&self, _call: xai_grok_sampler::BilledCall) {}
}

fn required_env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set to run a live embedding test"))
}

fn live_provider() -> ApiEmbeddingProvider {
    let dimensions = std::env::var("GROK_LIVE_EMBEDDING_DIMENSIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(xai_grok_config_types::MemoryEmbeddingConfig::default().dimensions);
    ApiEmbeddingProvider::new(
        required_env("GROK_LIVE_EMBEDDING_BASE_URL"),
        required_env("GROK_LIVE_EMBEDDING_MODEL"),
        dimensions,
        xai_grok_net::Auth::ApiKey {
            key: "live-test".into(),
            scheme: xai_grok_net::KeyScheme::Bearer,
        },
        std::sync::Arc::new(NoBilling),
    )
}

/// A note as capture writes it: frontmatter, then the statement as the title.
fn note(kind: &str, statement: &str, keywords: &[&str]) -> String {
    format!(
        "---\nschema_version: 2\ntype: {kind}\ntopic_hint: \"capture\"\nkeywords: {}\n\
         aliases: []\nsession_id: \"sess-live\"\nfrom_turn: 1\nthrough_turn: 1\n\
         extraction_model: \"extractor\"\nprompt_version: \"v2\"\ncreated_at: 5\n\
         job_id: \"job-live\"\nobservation_ordinal: 1\nobservation_count: 1\n---\n\n# {statement}\n",
        serde_json::to_string(keywords).unwrap(),
    )
}

/// `(file stem, note)`. Each stem names what a matching query is about.
fn corpus() -> Vec<(&'static str, String)> {
    vec![
        (
            "release",
            note(
                "reference",
                "Release tooling lives in tools/ship. `make quokka-release` cuts a build and uploads it.",
                &["release", "ship", "tooling"],
            ),
        ),
        (
            "indent",
            note(
                "user",
                "The user wants every indent to be a tab, never spaces.",
                &["indent", "tabs", "whitespace"],
            ),
        ),
        (
            "ci-hang",
            note(
                "project",
                "CI runners have no init process, so a hung child is never reaped and PTY tests stall there.",
                &["ci", "runner", "pty"],
            ),
        ),
        (
            "staging-db",
            note(
                "reference",
                "Staging Postgres credentials come from the vault path secret/staging/db. Never hardcode them.",
                &["postgres", "staging", "vault"],
            ),
        ),
        (
            "branches",
            note(
                "feedback",
                "Feature branches are named <ticket>-<slug>. The user rejected a branch named after a date.",
                &["branch", "naming", "git"],
            ),
        ),
        (
            "darwin",
            note(
                "project",
                "The darwin binary is compiled on Linux and linked on macOS. An arm64 link needs the SDK sysroot.",
                &["darwin", "macos", "arm64"],
            ),
        ),
        (
            "keymaps",
            note(
                "user",
                "The user edits with Vim keymaps and uses the leader key to switch buffers.",
                &["vim", "keymaps"],
            ),
        ),
        (
            "pricing",
            note(
                "project",
                "Model prices come from the pricing catalog request. Nothing prices an Anthropic model from the wire.",
                &["pricing", "catalog", "cost"],
            ),
        ),
        (
            "comments",
            note(
                "feedback",
                "The user wants code comments short and in the present tense, with no history of the change.",
                &["comments", "style"],
            ),
        ),
        (
            "sourdough",
            note(
                "user",
                "The sourdough starter needs flour and water every day.",
                &["sourdough", "starter"],
            ),
        ),
    ]
}

/// `(query, stem of the note that answers it)`. No query repeats its note's distinctive words.
const PARAPHRASES: &[(&str, &str)] = &[
    (
        "Where do we keep the scripts that publish a new version of the product?",
        "release",
    ),
    (
        "Please reformat this file with hard tab characters the way I like it.",
        "indent",
    ),
    (
        "The automated checks on the build machines keep freezing until they time out.",
        "ci-hang",
    ),
    (
        "How do I log in to the pre-production database from my laptop?",
        "staging-db",
    ),
    (
        "What should I call the new git branch for this ticket?",
        "branches",
    ),
    ("The app will not link for Apple silicon laptops.", "darwin"),
    (
        "User: the automated checks keep freezing on the build machines, can you look?\n\
         Assistant: I will read the workflow file and the last failed job log.\n\
         Tool call grep: {\"pattern\":\"timeout\",\"path\":\".github/workflows\"}",
        "ci-hang",
    ),
];

/// Turns that no stored note is about.
const UNRELATED: &[&str] = &[
    "Thanks, that looks good.",
    "Rename this variable to count.",
    "Write a haiku about autumn.",
    "What is the capital of France?",
    "Add a retry to the HTTP client when the server answers 503.",
    "Fix the typo in the README heading.",
    "User: add a retry to the HTTP client\nAssistant: I will wrap the request in a loop.\n\
     Tool call edit: {\"path\":\"src/http/client.rs\"}",
];

struct LiveFixture {
    _temp: TempDir,
    recall_dirs: (std::path::PathBuf, std::path::PathBuf),
}

fn live_fixture() -> LiveFixture {
    let temp = TempDir::new().unwrap();
    let global = temp.path().join("global");
    let workspace = temp.path().join("workspace");
    ensure_scope_initialized(temp.path(), &global, V2MemoryScope::Global).unwrap();
    ensure_scope_initialized(temp.path(), &workspace, V2MemoryScope::Workspace).unwrap();
    for (stem, content) in corpus() {
        let path = workspace.join(format!("observations/_inbox/{stem}.md"));
        std::fs::write(path, content).unwrap();
    }
    LiveFixture {
        _temp: temp,
        recall_dirs: (global, workspace),
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned()
}

/// Every chunk's standard score for `query`, best first, with the lexical path off.
async fn standard_scores(
    fixture: &LiveFixture,
    provider: &dyn EmbeddingProvider,
    query: &str,
) -> Vec<(f64, String)> {
    let recall = V2Recall::new(
        &fixture.recall_dirs.0,
        &fixture.recall_dirs.1,
        V2RecallConfig {
            max_hits: 64,
            min_matched_terms: usize::MAX,
            min_standard_score: f64::NEG_INFINITY,
            ..V2RecallConfig::default()
        },
    );
    let outcome = recall
        .recall(query, Some(provider), &HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        outcome.mode,
        V2RecallMode::Hybrid,
        "the embedding call failed"
    );
    let mut scores: Vec<(f64, String)> = outcome
        .hits
        .iter()
        .filter_map(|hit| Some((hit.standard_score?, stem(&hit.path))))
        .collect();
    scores.sort_by(|left, right| right.0.total_cmp(&left.0));
    scores
}

/// Prints the standard score of each query against the corpus, to see how a model separates.
#[tokio::test]
#[ignore = "needs a served embedding model"]
async fn live_standard_score_table() {
    let fixture = live_fixture();
    let provider = live_provider();
    println!("model {}", provider.model_name());
    for (query, expected) in PARAPHRASES {
        let scores = standard_scores(&fixture, &provider, query).await;
        let line: Vec<String> = scores
            .iter()
            .take(3)
            .map(|(score, stem)| format!("{stem} {score:.3}"))
            .collect();
        println!("match    want {expected:<10} got {}", line.join(" | "));
    }
    for query in UNRELATED {
        let scores = standard_scores(&fixture, &provider, query).await;
        let line: Vec<String> = scores
            .iter()
            .take(3)
            .map(|(score, stem)| format!("{stem} {score:.3}"))
            .collect();
        println!("no match                 got {}", line.join(" | "));
    }
}

/// The default config recalls the right note for a paraphrase and nothing for an unrelated turn.
#[tokio::test]
#[ignore = "needs a served embedding model"]
async fn live_model_recalls_a_paraphrase_and_stays_quiet_otherwise() {
    let fixture = live_fixture();
    let provider = live_provider();
    let recall = V2Recall::new(
        &fixture.recall_dirs.0,
        &fixture.recall_dirs.1,
        V2RecallConfig {
            // The lexical path is off, so each hit below comes from the model alone.
            min_matched_terms: usize::MAX,
            ..V2RecallConfig::default()
        },
    );
    let mut failures = Vec::new();
    for (query, expected) in PARAPHRASES {
        let outcome = recall
            .recall(query, Some(&provider), &HashSet::new())
            .await
            .unwrap();
        assert_eq!(outcome.mode, V2RecallMode::Hybrid);
        let got: Vec<String> = outcome.hits.iter().map(|hit| stem(&hit.path)).collect();
        if got.first().map(String::as_str) != Some(*expected) {
            failures.push(format!("`{query}` wanted {expected} first, got {got:?}"));
        }
    }
    for query in UNRELATED {
        let outcome = recall
            .recall(query, Some(&provider), &HashSet::new())
            .await
            .unwrap();
        assert_eq!(outcome.mode, V2RecallMode::Hybrid);
        if !outcome.hits.is_empty() {
            let got: Vec<String> = outcome
                .hits
                .iter()
                .map(|hit| format!("{} {:.3}", stem(&hit.path), hit.score))
                .collect();
            failures.push(format!("`{query}` wanted no note, got {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "model {}:\n{}",
        provider.model_name(),
        failures.join("\n")
    );
}
