use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tempfile::TempDir;

use super::*;
use crate::v2::ensure_scope_initialized;

struct Fixture {
    _temp: TempDir,
    global: PathBuf,
    workspace: PathBuf,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let global = temp.path().join("global");
    let workspace = temp.path().join("workspace");
    ensure_scope_initialized(temp.path(), &global, V2MemoryScope::Global).unwrap();
    ensure_scope_initialized(temp.path(), &workspace, V2MemoryScope::Workspace).unwrap();
    Fixture {
        _temp: temp,
        global,
        workspace,
    }
}

impl Fixture {
    fn recall(&self, config: V2RecallConfig) -> V2Recall {
        V2Recall::new(&self.global, &self.workspace, config)
    }
}

fn write(dir: &Path, relative: &str, content: &str) -> PathBuf {
    let path = dir.join(relative);
    std::fs::write(&path, content).unwrap();
    path
}

fn seed_topics(dir: &Path) {
    write(
        dir,
        "topics/build.md",
        "# Build cache\nThese memory notes cover the build.\n\n## Sccache\nThe sccache daemon stores compiled artifacts keyed by the rustc hash.\n",
    );
    write(
        dir,
        "topics/cooking.md",
        "# Cooking\nThese memory notes cover recipes.\n\n## Sourdough\nThe sourdough starter needs flour and water daily.\n",
    );
    write(
        dir,
        "topics/editor.md",
        "# Editor\nThese memory notes cover the editor.\n\n## Keymaps\nVim keymaps use the leader key for buffers.\n",
    );
}

fn observation(statement: &str, keywords: &[&str], aliases: &[&str], body: Option<&str>) -> String {
    let mut rendered = format!(
        "---\nschema_version: 2\ntype: project\ntopic_hint: \"capture\"\nkeywords: {}\n\
         aliases: {}\nsession_id: \"sess-zebrafish-4471\"\nfrom_turn: 1\nthrough_turn: 2\n\
         extraction_model: \"grok-extractor-quasar\"\nprompt_version: \"v7\"\ncreated_at: 5\n\
         job_id: \"job-marmalade\"\nobservation_ordinal: 1\nobservation_count: 1\n---\n\n# {statement}\n",
        serde_json::to_string(keywords).unwrap(),
        serde_json::to_string(aliases).unwrap(),
    );
    if let Some(body) = body {
        rendered.push('\n');
        rendered.push_str(body);
        rendered.push('\n');
    }
    rendered
}

async fn lexical(recall: &V2Recall, query: &str) -> V2RecallOutcome {
    recall.recall(query, None, &HashSet::new()).await.unwrap()
}

#[tokio::test]
async fn topic_matching_two_distinctive_terms_is_returned() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = lexical(&recall, "why does sccache rebuild after a rustc upgrade").await;

    assert_eq!(outcome.mode, V2RecallMode::Lexical);
    assert_eq!(outcome.corpus_chunks, 3);
    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    let hit = &outcome.hits[0];
    assert_eq!(hit.kind, V2RecallKind::Topic);
    assert_eq!(hit.title, "Build cache");
    assert_eq!(hit.path, fixture.workspace.join("topics/build.md"));
    assert_eq!(hit.matched_terms, vec!["rustc", "sccache"]);
    assert!(!hit.is_semantic);
    assert!(hit.score > 0.0 && hit.score <= 1.0);
    assert_eq!((hit.start_line, hit.end_line), (1, 5));
    assert_eq!(hit.key.len(), 16);
}

#[tokio::test]
async fn one_shared_term_does_not_qualify() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = lexical(&recall, "the sourdough compiler").await;

    assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
}

#[tokio::test]
async fn terms_in_every_chunk_carry_no_weight() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = lexical(&recall, "memory notes cover").await;

    assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
}

#[tokio::test]
async fn observation_frontmatter_terms_are_searchable_but_bookkeeping_is_not() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let path = write(
        &fixture.workspace,
        "observations/_inbox/obs.md",
        &observation(
            "Releases ship on Thursdays",
            &["cadence", "weekly_train"],
            &["shipday"],
            None,
        ),
    );
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = lexical(&recall, "what is the cadence of the weekly train shipday").await;
    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    assert_eq!(outcome.hits[0].path, path);
    assert_eq!(outcome.hits[0].kind, V2RecallKind::Observation);
    assert_eq!(
        outcome.hits[0].matched_terms,
        vec!["cadence", "shipday", "train", "weekly"]
    );

    let outcome = lexical(
        &recall,
        "sess-zebrafish-4471 grok-extractor-quasar job-marmalade schema_version observation_ordinal",
    )
    .await;
    assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
}

#[tokio::test]
async fn observation_hit_has_statement_title_and_real_line_range() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let content = observation(
        "Releases ship on Thursdays",
        &["cadence"],
        &[],
        Some("The release   \n\n\n\ntrain leaves at noon."),
    );
    write(&fixture.workspace, "observations/_inbox/obs.md", &content);
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = lexical(&recall, "releases thursdays").await;

    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    let hit = &outcome.hits[0];
    assert_eq!(hit.title, "Releases ship on Thursdays");
    assert_eq!(
        hit.snippet,
        "# Releases ship on Thursdays\n\nThe release\n\ntrain leaves at noon."
    );
    let lines: Vec<&str> = content.lines().collect();
    let frontmatter_lines = lines
        .iter()
        .skip(1)
        .position(|line| *line == "---")
        .unwrap()
        + 2;
    assert!(hit.start_line > frontmatter_lines);
    assert_eq!(lines[hit.start_line - 1], "# Releases ship on Thursdays");
    assert_eq!(lines[hit.end_line - 1], "train leaves at noon.");
    assert_eq!(hit.end_line, lines.len());
}

#[tokio::test]
async fn snippet_is_cut_on_a_char_boundary() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    write(
        &fixture.workspace,
        "topics/unicode.md",
        "# Glyphs\nÆther ünïcödé glyphs render through the harfbuzz shaper ééééééééé.\n",
    );
    let recall = fixture.recall(V2RecallConfig {
        snippet_chars: 12,
        ..V2RecallConfig::default()
    });

    let outcome = lexical(&recall, "harfbuzz shaper").await;

    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].snippet, "# Glyphs\nÆt…");
    assert_eq!(outcome.hits[0].snippet.chars().count(), 12);
}

#[tokio::test]
async fn seen_keys_are_skipped_without_using_up_max_hits() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    write(
        &fixture.workspace,
        "topics/build2.md",
        "# Remote cache\nThe sccache bucket shares artifacts; rustc upgrades invalidate it.\n",
    );
    let recall = fixture.recall(V2RecallConfig {
        max_hits: 1,
        ..V2RecallConfig::default()
    });

    let first = lexical(&recall, "sccache rustc artifacts").await;
    assert_eq!(first.hits.len(), 1);
    let seen: HashSet<String> = first.hits.iter().map(|hit| hit.key.clone()).collect();

    let second = recall
        .recall("sccache rustc artifacts", None, &seen)
        .await
        .unwrap();
    assert_eq!(second.hits.len(), 1);
    assert_ne!(second.hits[0].key, first.hits[0].key);
    assert_ne!(second.hits[0].path, first.hits[0].path);
}

#[tokio::test]
async fn edited_file_gets_a_new_key_and_deleted_file_leaves_the_corpus() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let path = write(
        &fixture.workspace,
        "topics/deploy.md",
        "# Deploy\nThe canary rollout uses helm charts.\n",
    );
    let recall = fixture.recall(V2RecallConfig::default());

    let before = lexical(&recall, "canary helm").await;
    assert_eq!(before.hits.len(), 1);
    assert_eq!(before.corpus_chunks, 4);

    std::fs::write(
        &path,
        "# Deploy\nThe blue green rollout now uses kustomize overlays and argocd.\n",
    )
    .unwrap();
    let after = lexical(&recall, "kustomize argocd").await;
    assert_eq!(after.hits.len(), 1);
    assert_ne!(after.hits[0].key, before.hits[0].key);
    assert!(lexical(&recall, "canary helm").await.hits.is_empty());

    std::fs::remove_file(&path).unwrap();
    let gone = lexical(&recall, "kustomize argocd").await;
    assert!(gone.hits.is_empty());
    assert_eq!(gone.corpus_chunks, 3);
}

#[tokio::test]
async fn hidden_observation_is_never_returned() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    crate::V2CaptureStore::open(&fixture.workspace, V2MemoryScope::Workspace).unwrap();
    write(
        &fixture.workspace,
        "observations/_inbox/hidden.md",
        "# Hidden secret about the flamingo ledger\n",
    );
    let visible = write(
        &fixture.workspace,
        "observations/_inbox/visible.md",
        "# Visible note about the flamingo ledger\n",
    );
    let state_path = fixture.workspace.join("memory_state.sqlite");
    let connection = xai_sqlite_journal::JournalMode::for_db_path(&state_path)
        .open(&state_path)
        .unwrap();
    connection
        .execute(
            "INSERT INTO memory_v2_hidden_observations(relative_path)
             VALUES ('observations/_inbox/hidden.md')",
            [],
        )
        .unwrap();
    drop(connection);
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = lexical(&recall, "flamingo ledger").await;

    assert_eq!(outcome.corpus_chunks, 4);
    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    assert_eq!(outcome.hits[0].path, visible);
}

struct TestProvider {
    texts: AtomicUsize,
}

#[async_trait]
impl crate::embedding::EmbeddingProvider for TestProvider {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        self.texts.fetch_add(texts.len(), Ordering::SeqCst);
        Ok(texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                if text.contains("lighthouse") || text.contains("beacon") {
                    vec![1.0, 0.1, 0.0]
                } else {
                    vec![0.0, 1.0, 0.0]
                }
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "test-embed"
    }

    fn dimensions(&self) -> usize {
        3
    }
}

struct FailingProvider;

#[async_trait]
impl crate::embedding::EmbeddingProvider for FailingProvider {
    async fn embed_batch(
        &self,
        _texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        Err("embedding service offline".into())
    }

    fn model_name(&self) -> &str {
        "test-embed"
    }

    fn dimensions(&self) -> usize {
        3
    }
}

#[tokio::test]
async fn semantic_hit_without_lexical_overlap_and_cached_vectors() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let path = write(
        &fixture.global,
        "topics/coast.md",
        "# Lighthouse\nThe keeper trims the lamp wick nightly.\n",
    );
    let provider = TestProvider {
        texts: AtomicUsize::new(0),
    };
    let recall = fixture.recall(V2RecallConfig::default());
    let query = "a beacon guiding ships";

    let outcome = recall
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();

    assert_eq!(outcome.mode, V2RecallMode::Hybrid);
    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    let hit = &outcome.hits[0];
    assert_eq!(hit.path, path);
    assert_eq!(hit.scope, V2MemoryScope::Global);
    assert!(hit.is_semantic);
    assert!(hit.matched_terms.is_empty());
    assert!(hit.score >= 0.45 && hit.score <= 1.0);
    assert_eq!(provider.texts.load(Ordering::SeqCst), 4 + 1);

    let again = recall
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();
    assert_eq!(again.hits, outcome.hits);
    assert_eq!(provider.texts.load(Ordering::SeqCst), 4 + 1 + 1);

    let fresh = fixture.recall(V2RecallConfig::default());
    fresh
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        provider.texts.load(Ordering::SeqCst),
        4 + 1 + 1 + 1,
        "a new recall must read the vectors from index.sqlite"
    );
}

#[tokio::test]
async fn hit_on_both_paths_gets_the_bonus() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    write(
        &fixture.workspace,
        "topics/coast.md",
        "# Lighthouse\nThe lighthouse keeper trims the lamp wick nightly.\n",
    );
    let provider = TestProvider {
        texts: AtomicUsize::new(0),
    };
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = recall
        .recall("lighthouse keeper", Some(&provider), &HashSet::new())
        .await
        .unwrap();

    assert_eq!(outcome.hits.len(), 1);
    assert!(outcome.hits[0].is_semantic);
    assert_eq!(outcome.hits[0].matched_terms, vec!["keeper", "lighthouse"]);
    assert_eq!(outcome.hits[0].score, 1.0);
}

#[tokio::test]
async fn failing_provider_falls_back_to_lexical_hits() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let recall = fixture.recall(V2RecallConfig::default());

    let outcome = recall
        .recall("sccache rustc", Some(&FailingProvider), &HashSet::new())
        .await
        .unwrap();

    match &outcome.mode {
        V2RecallMode::EmbeddingFailed(error) => {
            assert!(error.contains("embedding service offline"), "{error}");
        }
        other => panic!("expected EmbeddingFailed, got {other:?}"),
    }
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].title, "Build cache");
    assert!(!outcome.hits[0].is_semantic);
}

#[tokio::test]
async fn both_scopes_are_searched() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let global_path = write(
        &fixture.global,
        "topics/style.md",
        "# Style\nPrefer tabs for indentation in makefiles.\n",
    );
    let recall = fixture.recall(V2RecallConfig::default());

    let global = lexical(&recall, "tabs makefiles").await;
    assert_eq!(global.corpus_chunks, 4);
    assert_eq!(global.hits.len(), 1);
    assert_eq!(global.hits[0].scope, V2MemoryScope::Global);
    assert_eq!(global.hits[0].path, global_path);

    let workspace = lexical(&recall, "sourdough flour").await;
    assert_eq!(workspace.hits.len(), 1);
    assert_eq!(workspace.hits[0].scope, V2MemoryScope::Workspace);
}

#[tokio::test]
async fn missing_scope_directory_is_empty() {
    let temp = TempDir::new().unwrap();
    let recall = V2Recall::new(
        temp.path().join("absent-global"),
        temp.path().join("absent-workspace"),
        V2RecallConfig::default(),
    );

    let outcome = lexical(&recall, "anything at all").await;

    assert_eq!(outcome.corpus_chunks, 0);
    assert!(outcome.hits.is_empty());
}

#[tokio::test]
async fn chunk_cap_keeps_topics_then_newest_observations() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let old = write(
        &fixture.workspace,
        "observations/_inbox/old.md",
        "# Old note about the pelican harbor\n",
    );
    let new = write(
        &fixture.workspace,
        "observations/_inbox/new.md",
        "# New note about the walrus glacier\n",
    );
    filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(1_000_000, 0)).unwrap();
    filetime::set_file_mtime(&new, filetime::FileTime::from_unix_time(2_000_000, 0)).unwrap();
    let recall = fixture.recall(V2RecallConfig {
        max_chunks: 4,
        ..V2RecallConfig::default()
    });

    let outcome = lexical(&recall, "walrus glacier").await;
    assert_eq!(outcome.corpus_chunks, 4);
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].path, new);
    assert!(lexical(&recall, "pelican harbor").await.hits.is_empty());
    assert_eq!(lexical(&recall, "sccache rustc").await.hits.len(), 1);

    let topics_only = fixture.recall(V2RecallConfig {
        max_chunks: 3,
        ..V2RecallConfig::default()
    });
    assert!(
        lexical(&topics_only, "walrus glacier")
            .await
            .hits
            .is_empty()
    );
    assert_eq!(lexical(&topics_only, "sourdough flour").await.hits.len(), 1);
}
