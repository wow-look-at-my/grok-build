// v2_recall_tests.rs holds CrowdedProvider.

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

fn top(outcome: &V2RecallOutcome) -> &V2RecallHit {
    outcome.hits.first().unwrap()
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
    let hit = top(&outcome);
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
    assert_eq!(top(&outcome).path, path);
    assert_eq!(top(&outcome).kind, V2RecallKind::Observation);
    assert_eq!(
        top(&outcome).matched_terms,
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
async fn an_observation_of_the_asking_session_is_never_returned() {
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
    let text = "what is the cadence of the weekly train shipday";
    let asked_by = |session_id| V2RecallQuery {
        text,
        focus: Some(text),
        session_id: Some(session_id),
    };

    // The observation fixture names this session as its source.
    let own = recall
        .recall(asked_by("sess-zebrafish-4471"), None, &HashSet::new())
        .await
        .unwrap();
    assert!(
        own.hits.is_empty(),
        "a note captured from this conversation repeats it: {:?}",
        own.hits
    );

    let other = recall
        .recall(asked_by("sess-another"), None, &HashSet::new())
        .await
        .unwrap();
    assert_eq!(top(&other).path, path);
}

#[tokio::test]
async fn words_that_only_the_body_shares_do_not_recall_a_note() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let path = write(
        &fixture.workspace,
        "topics/fonts.md",
        "# Glyph shaping\nThe harfbuzz shaper renders ligatures through the freetype rasterizer.\n",
    );
    let recall = fixture.recall(V2RecallConfig::default());

    let body_only = lexical(&recall, "harfbuzz freetype rasterizer ligatures").await;
    assert!(body_only.hits.is_empty(), "{:?}", body_only.hits);

    let names_subject = lexical(&recall, "glyph shaping with harfbuzz").await;
    assert_eq!(top(&names_subject).path, path);
}

#[tokio::test]
async fn a_file_is_returned_one_time_for_a_conversation() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let filler = "The mirror keeps one bucket for each region. ".repeat(30);
    let path = write(
        &fixture.workspace,
        "topics/mirror.md",
        &format!(
            "# Sccache mirror\n\n## Upload\nThe rustc wrapper uploads objects. {filler}\n\n\
             ## Eviction\nThe rustc hash names each object. {filler}\n"
        ),
    );
    let recall = fixture.recall(V2RecallConfig {
        max_hits: 8,
        ..V2RecallConfig::default()
    });
    let query = "sccache mirror rustc";

    let first = lexical(&recall, query).await;
    assert!(first.corpus_chunks > 4, "the file must hold several chunks");
    let from_file: Vec<&V2RecallHit> = first.hits.iter().filter(|hit| hit.path == path).collect();
    assert_eq!(from_file.len(), 1, "{:?}", first.hits);

    let seen: HashSet<String> = first.hits.iter().map(|hit| hit.key.clone()).collect();
    let second = recall.recall(query, None, &seen).await.unwrap();
    assert!(
        second.hits.iter().all(|hit| hit.path != path),
        "the first reminder named the file: {:?}",
        second.hits
    );
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
    let hit = top(&outcome);
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
    assert_eq!(
        lines.get(hit.start_line - 1),
        Some(&"# Releases ship on Thursdays")
    );
    assert_eq!(lines.get(hit.end_line - 1), Some(&"train leaves at noon."));
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
    assert_eq!(top(&outcome).snippet, "# Glyphs\nÆt…");
    assert_eq!(top(&outcome).snippet.chars().count(), 12);
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
    assert_ne!(top(&second).key, top(&first).key);
    assert_ne!(top(&second).path, top(&first).path);
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
    assert_ne!(top(&after).key, top(&before).key);
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
    assert_eq!(top(&outcome).path, visible);
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
    let hit = top(&outcome);
    assert_eq!(hit.path, path);
    assert_eq!(hit.scope, V2MemoryScope::Global);
    assert!(hit.is_semantic);
    assert!(hit.matched_terms.is_empty());
    assert!(hit.standard_score.is_some_and(|score| score >= 3.0));
    assert_eq!(hit.score, 1.0);
    // The first call embeds the baseline notes, the chunks and the query.
    let first_call = BASELINE_NOTES.len() + 4 + 1;
    assert_eq!(provider.texts.load(Ordering::SeqCst), first_call);

    let again = recall
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();
    assert_eq!(again.hits, outcome.hits);
    assert_eq!(provider.texts.load(Ordering::SeqCst), first_call + 1);

    let fresh = fixture.recall(V2RecallConfig::default());
    fresh
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        provider.texts.load(Ordering::SeqCst),
        first_call + 1 + 1,
        "a new recall must read the vectors from index.sqlite"
    );
}

/// Puts every text at a high cosine with every other text, as some real models do.
struct CrowdedProvider;

#[async_trait]
impl crate::embedding::EmbeddingProvider for CrowdedProvider {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        Ok(texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                if text.contains("lighthouse") || text.contains("beacon") {
                    vec![1.0, 0.0]
                } else {
                    vec![1.0, 0.3]
                }
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "crowded-embed"
    }

    fn dimensions(&self) -> usize {
        2
    }
}

#[tokio::test]
async fn a_high_cosine_alone_is_not_a_semantic_hit() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let path = write(
        &fixture.workspace,
        "topics/coast.md",
        "# Lighthouse\nThe keeper trims the lamp wick nightly.\n",
    );
    let recall = fixture.recall(V2RecallConfig::default());

    // Only one stands out from the baseline.
    let outcome = recall
        .recall(
            "a beacon guiding ships",
            Some(&CrowdedProvider),
            &HashSet::new(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.mode, V2RecallMode::Hybrid);
    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    assert_eq!(top(&outcome).path, path);

    // This query sits with the baseline, so no chunk stands out.
    let outcome = recall
        .recall(
            "thanks, that looks good",
            Some(&CrowdedProvider),
            &HashSet::new(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.mode, V2RecallMode::Hybrid);
    assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
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
    assert!(top(&outcome).is_semantic);
    assert_eq!(top(&outcome).matched_terms, vec!["keeper", "lighthouse"]);
    assert_eq!(top(&outcome).score, 1.0);
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
    assert_eq!(top(&outcome).title, "Build cache");
    assert!(!top(&outcome).is_semantic);
}

/// Fails one request, by its position, and answers the others as `TestProvider` does.
struct FlakyProvider {
    inner: TestProvider,
    requests: AtomicUsize,
    failing_request: usize,
}

#[async_trait]
impl crate::embedding::EmbeddingProvider for FlakyProvider {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        if self.requests.fetch_add(1, Ordering::SeqCst) + 1 == self.failing_request {
            return Err("embedding service timed out".into());
        }
        self.inner.embed_batch(texts).await
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

#[tokio::test]
async fn a_failed_request_keeps_the_vectors_of_the_requests_before_it() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let path = write(
        &fixture.global,
        "topics/coast.md",
        "# Lighthouse\nThe keeper trims the lamp wick nightly.\n",
    );
    let provider = FlakyProvider {
        inner: TestProvider {
            texts: AtomicUsize::new(0),
        },
        requests: AtomicUsize::new(0),
        failing_request: 2,
    };
    let config = V2RecallConfig::default();
    let batch = config.embed_batch_size;
    let recall = fixture.recall(config);
    let query = "a beacon guiding ships";

    let failed = recall
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();

    assert!(
        matches!(failed.mode, V2RecallMode::EmbeddingFailed(_)),
        "{:?}",
        failed.mode
    );
    assert_eq!(provider.inner.texts.load(Ordering::SeqCst), batch);

    let outcome = recall
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();

    assert_eq!(outcome.mode, V2RecallMode::Hybrid);
    assert_eq!(top(&outcome).path, path);
    assert!(top(&outcome).is_semantic);
    // The second recall embeds only what the first one did not store.
    assert_eq!(
        provider.inner.texts.load(Ordering::SeqCst),
        BASELINE_NOTES.len() + 4 + 1
    );
}

#[tokio::test]
async fn a_model_slower_than_the_time_budget_finishes_over_several_recalls() {
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
    let config = V2RecallConfig {
        embed_time_budget: Duration::ZERO,
        ..V2RecallConfig::default()
    };
    let batch = config.embed_batch_size;
    let recall = fixture.recall(config);
    let query = "a beacon guiding ships";

    // One request for each recall. No chunk is judged before the baseline has its vectors.
    let baseline_recalls = BASELINE_NOTES.len().div_ceil(batch);
    for done in 1..=baseline_recalls {
        let outcome = recall
            .recall(query, Some(&provider), &HashSet::new())
            .await
            .unwrap();
        assert_eq!(outcome.mode, V2RecallMode::Hybrid);
        assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
        let embedded = provider.texts.load(Ordering::SeqCst);
        assert!(embedded >= (done * batch).min(BASELINE_NOTES.len()));
    }

    let outcome = recall
        .recall(query, Some(&provider), &HashSet::new())
        .await
        .unwrap();

    assert_eq!(top(&outcome).path, path);
    assert!(top(&outcome).is_semantic);
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
    assert_eq!(top(&global).scope, V2MemoryScope::Global);
    assert_eq!(top(&global).path, global_path);

    let workspace = lexical(&recall, "sourdough flour").await;
    assert_eq!(workspace.hits.len(), 1);
    assert_eq!(top(&workspace).scope, V2MemoryScope::Workspace);
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
    assert_eq!(top(&outcome).path, new);
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

#[tokio::test]
async fn a_long_query_matches_on_its_most_distinctive_terms() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    // The build words repeat. Every other topic has many words in the query too.
    let query = "sccache rustc sccache rustc sourdough starter flour water recipes cooking \
                 vim keymaps leader buffers editor";

    let every_term = fixture.recall(V2RecallConfig {
        max_query_terms: usize::MAX,
        ..V2RecallConfig::default()
    });
    assert_eq!(
        lexical(&every_term, query).await.hits.len(),
        3,
        "without the cut, a query with enough words matches every topic"
    );

    let distinctive = fixture.recall(V2RecallConfig {
        max_query_terms: 2,
        ..V2RecallConfig::default()
    });
    let outcome = lexical(&distinctive, query).await;
    assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
    assert!(top(&outcome).path.ends_with("topics/build.md"));
    assert_eq!(top(&outcome).matched_terms, vec!["rustc", "sccache"]);
}

#[tokio::test]
async fn a_cached_vector_leaves_with_its_chunk() {
    let fixture = fixture();
    seed_topics(&fixture.workspace);
    let provider = TestProvider {
        texts: AtomicUsize::new(0),
    };
    let recall = fixture.recall(V2RecallConfig::default());
    let cached_rows = || {
        rusqlite::Connection::open(fixture.workspace.join("index.sqlite"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM recall_embeddings", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };

    recall
        .recall("a beacon guiding ships", Some(&provider), &HashSet::new())
        .await
        .unwrap();
    // The baseline vectors share the cache of the scope that holds the chunks.
    let baseline_rows = i64::try_from(BASELINE_NOTES.len()).unwrap();
    assert_eq!(cached_rows(), baseline_rows + 3);

    std::fs::remove_file(fixture.workspace.join("topics/cooking.md")).unwrap();
    recall
        .recall("a beacon guiding ships", Some(&provider), &HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        cached_rows(),
        baseline_rows + 2,
        "the removed chunk's vector must be pruned"
    );
}
