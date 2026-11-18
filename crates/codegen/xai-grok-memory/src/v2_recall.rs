//! Memory-v2 recall: finds the topic sections and pending observations that
//! are relevant to a stretch of conversation text.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::Mutex;
use rusqlite::params;
use xai_grok_config_types::MemoryIndexConfig;
use xai_sqlite_journal::JournalMode;

use crate::MEMORY_LOG_TARGET;
use crate::chunker::{chunk_hash, chunk_markdown};
use crate::embedding::EmbeddingProvider;
use crate::query_expansion::extract_keywords;
use crate::v2::{
    MAX_DIRECTORY_ENTRIES, MAX_MANUAL_OBSERVATION_BYTES, V2MemoryScope, V2StorageError,
    excluded_manifest_paths,
};

const MAX_FILE_BYTES: u64 = MAX_MANUAL_OBSERVATION_BYTES as u64;
const QUERY_EMBED_CHARS: usize = 4000;
const BOTH_PATHS_BONUS: f64 = 0.1;
const KEY_HEX_CHARS: usize = 16;
/// A standard score at or above this ranks as a full semantic match.
const FULL_MATCH_STANDARD_SCORE: f64 = 8.0;
/// Fewer baseline vectors than this give no usable spread.
const MIN_BASELINE_VECTORS: usize = 8;
/// Keeps a near-constant baseline from turning a small difference into a large standard score.
const MIN_BASELINE_SPREAD: f64 = 0.01;
/// Notes on varied software subjects. A query's similarity to them is what "unrelated" scores
/// with the model in use. A chunk is judged against them and not against a fixed cosine. Each
/// embedding model puts cosines on its own scale.
const BASELINE_NOTES: [&str; 24] = [
    "The parser keeps a lookahead of one token and reports the first error only.",
    "Dates are stored as UTC timestamps and rendered in the viewer local zone.",
    "The image resizer keeps the aspect ratio and writes WebP by default.",
    "Search results are paged with an opaque cursor, not with an offset.",
    "The mobile app caches the last feed response for offline reading.",
    "Log lines are JSON objects with a level, a message and a request id.",
    "The scheduler retries a failed job on a fixed interval.",
    "User avatars are served from the content delivery network with a one day lifetime.",
    "The spreadsheet export escapes a leading equals sign in each cell.",
    "The audio player crossfades two tracks over a few seconds.",
    "Feature flags are read once at process start.",
    "The map view clusters markers that overlap at the current zoom.",
    "Passwords are hashed with a memory-hard function before storage.",
    "The email template uses a single column layout for narrow screens.",
    "The cache evicts the least recently used entry when it is full.",
    "The game loop runs physics at a fixed step and renders as fast as it can.",
    "The invoice total is rounded to the nearest cent after tax.",
    "The chat widget reconnects its socket when the tab becomes visible.",
    "The compiler inlines a function that is called from one place.",
    "The recipe page converts between metric and imperial units.",
    "The backup job writes one archive per day and keeps thirty.",
    "The form validates a field when it loses focus.",
    "The translation files are keyed by the English source string.",
    "The thermostat firmware samples the sensor once a minute.",
];
const CREATE_CACHE_SQL: &str = "CREATE TABLE IF NOT EXISTS recall_embeddings (
    hash TEXT NOT NULL,
    model TEXT NOT NULL,
    dims INTEGER NOT NULL,
    vector BLOB NOT NULL,
    PRIMARY KEY (hash, model)
)";

/// The kind of memory file a recall hit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V2RecallKind {
    Topic,
    Observation,
}

impl V2RecallKind {
    fn rank(self) -> u8 {
        match self {
            Self::Topic => 0,
            Self::Observation => 1,
        }
    }
}

/// One stored memory chunk that is relevant to the recall query.
#[derive(Debug, Clone, PartialEq)]
pub struct V2RecallHit {
    /// First hex chars of blake3(absolute path + "\n" + chunk text). Changes when the chunk changes.
    pub key: String,
    pub scope: V2MemoryScope,
    pub kind: V2RecallKind,
    /// Absolute path of the `.md` file.
    pub path: PathBuf,
    /// The file's `# ` heading (the statement of an observation), else the file stem.
    pub title: String,
    /// 1-based inclusive line range of the chunk in the file.
    pub start_line: usize,
    pub end_line: usize,
    /// Normalized chunk text without frontmatter, cut to `snippet_chars`.
    pub snippet: String,
    /// Ranking score in `[0, 1]`.
    pub score: f64,
    /// Distinct query terms that matched, sorted.
    pub matched_terms: Vec<String>,
    /// True when the vector path qualified this hit.
    pub is_semantic: bool,
    /// The standard score that qualified a semantic hit.
    pub standard_score: Option<f64>,
}

/// Thresholds and limits for [`V2Recall`].
#[derive(Debug, Clone, PartialEq)]
pub struct V2RecallConfig {
    pub max_hits: usize,
    pub min_matched_terms: usize,
    /// Minimum summed idf of the matched terms, as a multiple of the idf of a term in one chunk.
    pub min_term_weight: f64,
    /// A semantic hit needs a cosine this many standard deviations above the query's mean cosine with `BASELINE_NOTES`.
    pub min_standard_score: f64,
    /// A second semantic hit needs this share of the best standard score in the same recall.
    pub runner_up_fraction: f64,
    pub snippet_chars: usize,
    pub max_chunks: usize,
    pub max_embed_per_call: usize,
    /// Word matching uses at most this many query terms, the most distinctive ones.
    pub max_query_terms: usize,
}

impl Default for V2RecallConfig {
    fn default() -> Self {
        Self {
            max_hits: 4,
            min_matched_terms: 2,
            min_term_weight: 1.5,
            min_standard_score: 3.5,
            runner_up_fraction: 0.6,
            snippet_chars: 320,
            max_chunks: 4096,
            max_embed_per_call: 64,
            max_query_terms: 24,
        }
    }
}

/// Which scoring paths took part in a recall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V2RecallMode {
    /// No embedding provider was given.
    Lexical,
    /// Vectors took part.
    Hybrid,
    /// A provider was given and failed; the hits are lexical only. Carries the error text.
    EmbeddingFailed(String),
}

/// What one recall searches with.
#[derive(Debug, Clone, Copy)]
pub struct V2RecallQuery<'a> {
    /// Every new piece of the conversation. Word matching reads all of it.
    pub text: &'a str,
    /// The last thing a person or the model said in `text`.
    pub focus: Option<&'a str>,
}

impl<'a> From<&'a str> for V2RecallQuery<'a> {
    fn from(text: &'a str) -> Self {
        Self {
            text,
            focus: Some(text),
        }
    }
}

/// The result of one [`V2Recall::recall`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct V2RecallOutcome {
    pub hits: Vec<V2RecallHit>,
    pub mode: V2RecallMode,
    /// Chunks in the corpus after the refresh.
    pub corpus_chunks: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum V2RecallError {
    #[error("failed to {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("recall embedding cache failed at {path}: {source}")]
    Database {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(transparent)]
    Storage(#[from] V2StorageError),
    #[error("recall task failed: {0}")]
    Task(String),
}

/// Finds relevant memory-v2 chunks. Cheap to clone; clones share the corpus cache.
#[derive(Clone)]
pub struct V2Recall {
    inner: Arc<Inner>,
}

struct Inner {
    global_dir: PathBuf,
    workspace_dir: PathBuf,
    config: V2RecallConfig,
    files: Mutex<HashMap<PathBuf, CachedFile>>,
}

struct CachedFile {
    modified: Option<SystemTime>,
    len: u64,
    chunks: Vec<Arc<CorpusChunk>>,
}

#[derive(Debug)]
struct CorpusChunk {
    key: String,
    scope: V2MemoryScope,
    kind: V2RecallKind,
    path: PathBuf,
    title: String,
    start_line: usize,
    end_line: usize,
    display: String,
    searchable: String,
    /// `chunk_hash` of `searchable`; the embedding cache key.
    hash: String,
    terms: HashSet<String>,
}

struct ListedFile {
    path: PathBuf,
    kind: V2RecallKind,
    modified: Option<SystemTime>,
    len: u64,
}

struct LexicalScore {
    score: Option<f64>,
    matched: Vec<String>,
}

impl V2Recall {
    pub fn new(
        global_dir: impl Into<PathBuf>,
        workspace_dir: impl Into<PathBuf>,
        config: V2RecallConfig,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                global_dir: global_dir.into(),
                workspace_dir: workspace_dir.into(),
                config,
                files: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Refresh the corpus from disk, then return the chunks relevant to `query`, best first.
    /// Chunks whose key is in `seen` are never returned and do not use up `max_hits`.
    pub async fn recall<'a>(
        &self,
        query: impl Into<V2RecallQuery<'a>>,
        provider: Option<&dyn EmbeddingProvider>,
        seen: &HashSet<String>,
    ) -> Result<V2RecallOutcome, V2RecallError> {
        let query: V2RecallQuery<'a> = query.into();
        let inner = Arc::clone(&self.inner);
        let query_counts = term_counts(query.text);
        let (chunks, lexical) = tokio::task::spawn_blocking(move || {
            let chunks = inner.refresh()?;
            let query_terms =
                distinctive_terms(&chunks, &query_counts, inner.config.max_query_terms);
            let lexical = lexical_scores(&chunks, &query_terms, &inner.config);
            Ok::<_, V2RecallError>((chunks, lexical))
        })
        .await
        .map_err(|error| V2RecallError::Task(error.to_string()))??;

        let focus = query.focus.filter(|focus| !focus.trim().is_empty());
        let (mode, semantic) = match provider.zip(focus) {
            None => (V2RecallMode::Lexical, vec![None; chunks.len()]),
            Some((provider, focus)) => match self.vector_scores(&chunks, focus, provider).await {
                Ok(scores) => (V2RecallMode::Hybrid, scores),
                Err(error) => {
                    tracing::warn!(
                        target: MEMORY_LOG_TARGET,
                        error = %error,
                        "memory recall embedding failed; using lexical hits only"
                    );
                    (
                        V2RecallMode::EmbeddingFailed(error),
                        vec![None; chunks.len()],
                    )
                }
            },
        };

        let config = &self.inner.config;
        let mut hits = Vec::new();
        for ((chunk, lexical), semantic) in chunks.iter().zip(lexical).zip(semantic) {
            let semantic_score =
                semantic.map(|standard| (standard / FULL_MATCH_STANDARD_SCORE).clamp(0.0, 1.0));
            let score = match (lexical.score, semantic_score) {
                (Some(lexical), Some(semantic)) => {
                    (lexical.max(semantic) + BOTH_PATHS_BONUS).min(1.0)
                }
                (Some(lexical), None) => lexical,
                (None, Some(semantic)) => semantic,
                (None, None) => continue,
            };
            if seen.contains(&chunk.key) {
                continue;
            }
            hits.push(V2RecallHit {
                key: chunk.key.clone(),
                scope: chunk.scope,
                kind: chunk.kind,
                path: chunk.path.clone(),
                title: chunk.title.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                snippet: snippet(&chunk.display, config.snippet_chars),
                score: score.clamp(0.0, 1.0),
                matched_terms: lexical.matched,
                is_semantic: semantic.is_some(),
                standard_score: semantic,
            });
        }
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.kind.rank().cmp(&right.kind.rank()))
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.start_line.cmp(&right.start_line))
        });
        hits.truncate(config.max_hits);
        Ok(V2RecallOutcome {
            hits,
            mode,
            corpus_chunks: chunks.len(),
        })
    }

    /// The standard score of each chunk, `Some` only when it reaches `min_standard_score`.
    async fn vector_scores(
        &self,
        chunks: &[Arc<CorpusChunk>],
        query: &str,
        provider: &dyn EmbeddingProvider,
    ) -> Result<Vec<Option<f64>>, String> {
        if chunks.is_empty() || query.trim().is_empty() {
            return Ok(vec![None; chunks.len()]);
        }
        let model = provider.model_name().to_owned();
        let scope_dirs = [
            self.inner.global_dir.clone(),
            self.inner.workspace_dir.clone(),
        ];
        // The baseline vectors live in the cache of a scope that holds a chunk.
        let baseline_slot = chunks
            .iter()
            .map(|chunk| scope_slot(chunk.scope))
            .min()
            .unwrap_or_default();
        let baseline: Vec<(String, &str)> = BASELINE_NOTES
            .iter()
            .map(|text| (chunk_hash(text), *text))
            .collect();
        let mut wanted: [HashSet<String>; 2] = Default::default();
        for chunk in chunks {
            if let Some(set) = wanted.get_mut(scope_slot(chunk.scope)) {
                set.insert(chunk.hash.clone());
            }
        }
        if let Some(set) = wanted.get_mut(baseline_slot) {
            set.extend(baseline.iter().map(|(hash, _)| hash.clone()));
        }

        let load_dirs = scope_dirs.clone();
        let load_model = model.clone();
        let mut cached: [HashMap<String, Vec<f32>>; 2] = tokio::task::spawn_blocking(move || {
            let [global, workspace] = wanted;
            let [global_dir, workspace_dir] = load_dirs;
            Ok::<_, V2RecallError>([
                load_cached_vectors(&global_dir, &load_model, &global)?,
                load_cached_vectors(&workspace_dir, &load_model, &workspace)?,
            ])
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

        // The baseline goes first: no chunk can be judged without it.
        let mut missing: Vec<(usize, &str, &str)> = Vec::new();
        let mut queued: HashSet<(usize, &str)> = HashSet::new();
        for (hash, text) in &baseline {
            let has_vector = cached
                .get(baseline_slot)
                .is_some_and(|vectors| vectors.contains_key(hash));
            if !has_vector && queued.insert((baseline_slot, hash.as_str())) {
                missing.push((baseline_slot, hash.as_str(), text));
            }
        }
        let chunk_budget = missing.len() + self.inner.config.max_embed_per_call;
        for chunk in chunks {
            if missing.len() >= chunk_budget {
                break;
            }
            let slot = scope_slot(chunk.scope);
            let has_vector = cached
                .get(slot)
                .is_some_and(|vectors| vectors.contains_key(&chunk.hash));
            if !has_vector && queued.insert((slot, chunk.hash.as_str())) {
                missing.push((slot, chunk.hash.as_str(), chunk.searchable.as_str()));
            }
        }

        if !missing.is_empty() {
            let texts: Vec<&str> = missing.iter().map(|(_, _, text)| *text).collect();
            let vectors = provider
                .embed_batch(&texts)
                .await
                .map_err(|error| format!("embedding memory chunks failed: {error}"))?;
            if vectors.len() != texts.len() {
                return Err(format!(
                    "embedding provider returned {} vectors for {} memory chunks",
                    vectors.len(),
                    texts.len()
                ));
            }
            let mut fresh: [Vec<(String, Vec<f32>)>; 2] = Default::default();
            for ((slot, hash, _), vector) in missing.iter().zip(vectors) {
                if vector.is_empty() {
                    continue;
                }
                if let Some(rows) = fresh.get_mut(*slot) {
                    rows.push(((*hash).to_owned(), vector.clone()));
                }
                if let Some(vectors) = cached.get_mut(*slot) {
                    vectors.insert((*hash).to_owned(), vector);
                }
            }
            let store_dirs = scope_dirs.clone();
            let store_model = model.clone();
            tokio::task::spawn_blocking(move || {
                let [global, workspace] = fresh;
                let [global_dir, workspace_dir] = store_dirs;
                store_vectors(&global_dir, &store_model, &global)?;
                store_vectors(&workspace_dir, &store_model, &workspace)
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        }

        let query_text = tail_chars(query, QUERY_EMBED_CHARS);
        let query_vector = provider
            .embed_batch(&[query_text])
            .await
            .map_err(|error| format!("embedding the recall query failed: {error}"))?
            .into_iter()
            .next()
            .filter(|vector| !vector.is_empty())
            .ok_or_else(|| "embedding provider returned no vector for the query".to_owned())?;

        let baseline_cosines: Vec<f64> = baseline
            .iter()
            .filter_map(|(hash, _)| cached.get(baseline_slot)?.get(hash))
            .filter(|vector| vector.len() == query_vector.len())
            .map(|vector| cosine(vector, &query_vector))
            .collect();
        if baseline_cosines.len() < MIN_BASELINE_VECTORS {
            return Err(format!(
                "the embedding model returned {} usable baseline vectors, and {} are needed",
                baseline_cosines.len(),
                MIN_BASELINE_VECTORS
            ));
        }
        let count = baseline_cosines.len() as f64;
        let mean = baseline_cosines.iter().sum::<f64>() / count;
        let variance = baseline_cosines
            .iter()
            .map(|cosine| (cosine - mean).powi(2))
            .sum::<f64>()
            / count;
        let spread = variance.sqrt().max(MIN_BASELINE_SPREAD);

        let standard_scores: Vec<Option<f64>> = chunks
            .iter()
            .map(|chunk| {
                let vector = cached
                    .get(scope_slot(chunk.scope))
                    .and_then(|vectors| vectors.get(&chunk.hash))
                    .filter(|vector| vector.len() == query_vector.len())?;
                Some((cosine(vector, &query_vector) - mean) / spread)
            })
            .collect();
        // A weaker chunk beside a strong one is its neighbor in the model, not a second match.
        let best = standard_scores
            .iter()
            .flatten()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let config = &self.inner.config;
        let floor = config
            .min_standard_score
            .max(best * config.runner_up_fraction);
        Ok(standard_scores
            .into_iter()
            .map(|score| score.filter(|score| *score >= floor))
            .collect())
    }
}

impl Inner {
    /// Re-read changed files and return the capped corpus: topics first, then observations newest first.
    fn refresh(&self) -> Result<Vec<Arc<CorpusChunk>>, V2RecallError> {
        let mut files = self.files.lock();
        let mut next = HashMap::new();
        let mut topics: Vec<Arc<CorpusChunk>> = Vec::new();
        let mut observations: Vec<(Option<SystemTime>, PathBuf, Vec<Arc<CorpusChunk>>)> =
            Vec::new();
        for (scope, scope_dir) in [
            (V2MemoryScope::Global, &self.global_dir),
            (V2MemoryScope::Workspace, &self.workspace_dir),
        ] {
            for listed in list_scope_files(scope_dir)? {
                let cached = match files.remove(&listed.path) {
                    Some(cached)
                        if cached.modified == listed.modified && cached.len == listed.len =>
                    {
                        cached
                    }
                    _ => match load_file(&listed.path, scope, listed.kind) {
                        Some(chunks) => CachedFile {
                            modified: listed.modified,
                            len: listed.len,
                            chunks,
                        },
                        None => continue,
                    },
                };
                match listed.kind {
                    V2RecallKind::Topic => topics.extend(cached.chunks.iter().cloned()),
                    V2RecallKind::Observation => observations.push((
                        listed.modified,
                        listed.path.clone(),
                        cached.chunks.clone(),
                    )),
                }
                next.insert(listed.path, cached);
            }
        }
        *files = next;
        drop(files);

        topics.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.start_line.cmp(&right.start_line))
        });
        observations.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let total = topics.len()
            + observations
                .iter()
                .map(|(_, _, chunks)| chunks.len())
                .sum::<usize>();
        let max_chunks = self.config.max_chunks;
        if total > max_chunks {
            tracing::warn!(
                target: MEMORY_LOG_TARGET,
                total,
                max_chunks,
                dropped = total - max_chunks,
                "memory recall corpus exceeds its chunk cap; dropping the oldest observation chunks"
            );
        }
        let corpus = topics
            .into_iter()
            .chain(observations.into_iter().flat_map(|(_, _, chunks)| chunks))
            .take(max_chunks)
            .collect();
        Ok(corpus)
    }
}

fn scope_slot(scope: V2MemoryScope) -> usize {
    match scope {
        V2MemoryScope::Global => 0,
        V2MemoryScope::Workspace => 1,
    }
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

fn list_scope_files(scope_dir: &Path) -> Result<Vec<ListedFile>, V2RecallError> {
    match std::fs::symlink_metadata(scope_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(V2RecallError::Io {
                operation: "inspect v2 memory scope",
                path: scope_dir.to_path_buf(),
                source,
            });
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            tracing::warn!(
                target: MEMORY_LOG_TARGET,
                path = %scope_dir.display(),
                "memory recall skips a symlinked scope directory"
            );
            return Ok(Vec::new());
        }
        Ok(metadata) if !metadata.is_dir() => return Ok(Vec::new()),
        Ok(_) => {}
    }
    let excluded = excluded_manifest_paths(scope_dir)?;

    let mut files = Vec::new();
    for (relative, kind) in [
        ("topics", V2RecallKind::Topic),
        ("observations/_inbox", V2RecallKind::Observation),
    ] {
        let directory = scope_dir.join(relative);
        let mut component = scope_dir.to_path_buf();
        let mut symlinked = false;
        for part in Path::new(relative).components() {
            component.push(part);
            symlinked |= is_symlink(&component);
        }
        if symlinked {
            tracing::warn!(
                target: MEMORY_LOG_TARGET,
                path = %directory.display(),
                "memory recall skips a symlinked memory directory"
            );
            continue;
        }
        let read_dir = match std::fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(V2RecallError::Io {
                    operation: "enumerate v2 memory files",
                    path: directory,
                    source,
                });
            }
        };
        for (index, entry) in read_dir.enumerate() {
            if index >= MAX_DIRECTORY_ENTRIES {
                tracing::warn!(
                    target: MEMORY_LOG_TARGET,
                    path = %directory.display(),
                    limit = MAX_DIRECTORY_ENTRIES,
                    "memory recall stopped at the directory entry limit; later files are not searched"
                );
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::warn!(
                        target: MEMORY_LOG_TARGET,
                        path = %directory.display(),
                        error = %error,
                        "memory recall could not read a directory entry"
                    );
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.file_type().is_symlink() {
                tracing::warn!(
                    target: MEMORY_LOG_TARGET,
                    path = %path.display(),
                    "memory recall skips a symlinked memory file"
                );
                continue;
            }
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            let is_excluded = path
                .strip_prefix(scope_dir)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .is_ok_and(|relative| excluded.contains(&relative));
            if is_excluded {
                continue;
            }
            files.push(ListedFile {
                path,
                kind,
                modified: metadata.modified().ok(),
                len: metadata.len(),
            });
        }
    }
    Ok(files)
}

/// Read and chunk one file. `None` when the file cannot be read now.
fn load_file(
    path: &Path,
    scope: V2MemoryScope,
    kind: V2RecallKind,
) -> Option<Vec<Arc<CorpusChunk>>> {
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes));
    if let Err(error) = read {
        tracing::warn!(
            target: MEMORY_LOG_TARGET,
            path = %path.display(),
            error = %error,
            "memory recall could not read a memory file"
        );
        return None;
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let content = String::from_utf8_lossy(&bytes);
    Some(build_chunks(path, scope, kind, &content))
}

fn build_chunks(
    path: &Path,
    scope: V2MemoryScope,
    kind: V2RecallKind,
    content: &str,
) -> Vec<Arc<CorpusChunk>> {
    let (line_offset, body, extras) = match kind {
        V2RecallKind::Observation => split_frontmatter(content),
        V2RecallKind::Topic => (0, content, Vec::new()),
    };
    let title = body
        .lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("memory")
                .to_owned()
        });
    let file_lines: Vec<&str> = content.lines().collect();
    let is_blank_line = |line: usize| {
        line.checked_sub(1)
            .and_then(|index| file_lines.get(index))
            .is_some_and(|text| text.trim().is_empty())
    };
    let path_text = path.to_string_lossy();

    let mut chunks = Vec::new();
    for chunk in chunk_markdown(body, &MemoryIndexConfig::default()) {
        if !has_content(&chunk.text, kind, &title) {
            continue;
        }
        let mut start_line = chunk.start_line + line_offset + 1;
        let mut end_line = chunk.end_line + line_offset;
        while start_line < end_line && is_blank_line(start_line) {
            start_line += 1;
        }
        while end_line > start_line && is_blank_line(end_line) {
            end_line -= 1;
        }
        end_line = end_line.max(start_line);

        let mut searchable = format!("{}\n{title}", chunk.text);
        for extra in &extras {
            searchable.push('\n');
            searchable.push_str(extra);
        }
        let key: String = blake3::hash(format!("{path_text}\n{}", chunk.text).as_bytes())
            .to_hex()
            .chars()
            .take(KEY_HEX_CHARS)
            .collect();
        chunks.push(Arc::new(CorpusChunk {
            key,
            scope,
            kind,
            path: path.to_path_buf(),
            title: title.clone(),
            start_line,
            end_line,
            hash: chunk_hash(&searchable),
            terms: tokenize(&searchable).into_iter().collect(),
            searchable,
            display: chunk.text,
        }));
    }
    chunks
}

/// Split a leading `---` block off. Other keys stay out of search.
fn split_frontmatter(content: &str) -> (usize, &str, Vec<String>) {
    let mut lines = content.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return (0, content, Vec::new());
    }
    let mut offset = lines_len_first(content);
    let mut extras = Vec::new();
    for (index, line) in lines.enumerate() {
        offset += line.len();
        let trimmed = line.trim_end();
        if trimmed == "---" {
            let body = content.get(offset..).unwrap_or_default();
            return (index + 2, body, extras);
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "keywords" | "aliases" => match serde_json::from_str::<Vec<String>>(value) {
                Ok(values) => extras.extend(values),
                Err(_) => extras.push(value.to_owned()),
            },
            "topic_hint" => match serde_json::from_str::<Option<String>>(value) {
                Ok(Some(hint)) => extras.push(hint),
                Ok(None) => {}
                Err(_) => extras.push(value.trim_matches('"').to_owned()),
            },
            _ => {}
        }
    }
    (0, content, Vec::new())
}

fn lines_len_first(content: &str) -> usize {
    content.split_inclusive('\n').next().map_or(0, str::len)
}

/// False when the chunk holds only headings and blank lines. An observation's statement heading counts as content.
fn has_content(text: &str, kind: V2RecallKind, title: &str) -> bool {
    text.lines().map(str::trim).any(|line| {
        !line.is_empty()
            && (!line.starts_with('#')
                || (kind == V2RecallKind::Observation
                    && line.strip_prefix("# ").map(str::trim) == Some(title)))
    })
}

/// Distinct keywords, plus the parts of each `_`-joined keyword.
fn tokenize(text: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for keyword in extract_keywords(text) {
        if keyword.contains('_') {
            terms.extend(extract_keywords(&keyword.replace('_', " ")));
        }
        terms.insert(keyword);
    }
    terms
}

/// How often each keyword of `text` occurs. A `_`-joined keyword also counts for its parts.
fn term_counts(text: &str) -> HashMap<String, usize> {
    let keywords: HashSet<String> = extract_keywords(text).into_iter().collect();
    let lowered = text.to_lowercase();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for word in lowered.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if !keywords.contains(word) {
            continue;
        }
        *counts.entry(word.to_owned()).or_default() += 1;
        if word.contains('_') {
            for part in extract_keywords(&word.replace('_', " ")) {
                *counts.entry(part).or_default() += 1;
            }
        }
    }
    counts
}

fn inverse_document_frequency(chunks: &[Arc<CorpusChunk>], term: &str) -> (usize, f64) {
    let total = chunks.len() as f64;
    let df = chunks
        .iter()
        .filter(|chunk| chunk.terms.contains(term))
        .count();
    let idf = (1.0 + (total - df as f64 + 0.5) / (df as f64 + 0.5)).ln();
    (df, idf)
}

/// The query terms that say the most about the query: frequent in it and rare in the corpus.
/// A long query holds so many words that the sum over all of them matches every chunk.
fn distinctive_terms(
    chunks: &[Arc<CorpusChunk>],
    query_counts: &HashMap<String, usize>,
    max_terms: usize,
) -> BTreeSet<String> {
    let mut ranked: Vec<(f64, &String)> = query_counts
        .iter()
        .filter_map(|(term, count)| {
            let (df, idf) = inverse_document_frequency(chunks, term);
            (df > 0).then(|| ((1.0 + (*count as f64).ln()) * idf, term))
        })
        .collect();
    ranked.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .take(max_terms)
        .map(|(_, term)| term.clone())
        .collect()
}

fn lexical_scores(
    chunks: &[Arc<CorpusChunk>],
    query_terms: &BTreeSet<String>,
    config: &V2RecallConfig,
) -> Vec<LexicalScore> {
    let total = chunks.len() as f64;
    let idf_max = (1.0 + (total - 0.5) / 1.5).ln();
    let idf: HashMap<&str, f64> = query_terms
        .iter()
        .map(|term| (term.as_str(), inverse_document_frequency(chunks, term).1))
        .collect();
    chunks
        .iter()
        .map(|chunk| {
            let matched: Vec<String> = query_terms
                .iter()
                .filter(|term| chunk.terms.contains(*term))
                .cloned()
                .collect();
            let weight: f64 = matched
                .iter()
                .filter_map(|term| idf.get(term.as_str()))
                .sum();
            let qualifies = idf_max > 0.0
                && matched.len() >= config.min_matched_terms
                && weight >= config.min_term_weight * idf_max;
            LexicalScore {
                score: qualifies.then(|| (weight / (3.0 * idf_max)).min(1.0)),
                matched,
            }
        })
        .collect()
}

fn snippet(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut blank_pending = false;
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() {
            blank_pending = !out.is_empty();
            continue;
        }
        if blank_pending {
            out.push_str("\n\n");
            blank_pending = false;
        } else if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    if out.chars().count() <= max_chars {
        return out;
    }
    let mut cut: String = out.chars().take(max_chars.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

fn tail_chars(text: &str, max_chars: usize) -> &str {
    match max_chars
        .checked_sub(1)
        .and_then(|skip| text.char_indices().rev().nth(skip))
    {
        Some((start, _)) => text.get(start..).unwrap_or(text),
        None => text,
    }
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    let (mut dot, mut left_norm, mut right_norm) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (a, b) in left.iter().zip(right) {
        let (a, b) = (f64::from(*a), f64::from(*b));
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}

fn open_cache(scope_dir: &Path) -> Result<(rusqlite::Connection, PathBuf), V2RecallError> {
    let path = scope_dir.join("index.sqlite");
    if is_symlink(&path) {
        return Err(V2RecallError::Io {
            operation: "open the recall embedding cache",
            path,
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "symbolic links are not allowed in memory v2",
            ),
        });
    }
    let database_error = |source| V2RecallError::Database {
        path: path.clone(),
        source,
    };
    let connection = JournalMode::for_db_path(&path)
        .open(&path)
        .map_err(database_error)?;
    connection
        .execute_batch(CREATE_CACHE_SQL)
        .map_err(database_error)?;
    Ok((connection, path))
}

fn load_cached_vectors(
    scope_dir: &Path,
    model: &str,
    wanted: &HashSet<String>,
) -> Result<HashMap<String, Vec<f32>>, V2RecallError> {
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let (connection, path) = open_cache(scope_dir)?;
    let database_error = |source| V2RecallError::Database {
        path: path.clone(),
        source,
    };
    let mut statement = connection
        .prepare("SELECT hash, vector FROM recall_embeddings WHERE model = ?1")
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![model], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(database_error)?;
    let mut vectors = HashMap::new();
    let mut stale = Vec::new();
    for row in rows {
        let (hash, bytes) = row.map_err(database_error)?;
        if !wanted.contains(&hash) {
            stale.push(hash);
            continue;
        }
        if bytes.is_empty() || bytes.len() % 4 != 0 {
            continue;
        }
        let vector = bytes
            .chunks_exact(4)
            .filter_map(|word| <[u8; 4]>::try_from(word).ok())
            .map(f32::from_le_bytes)
            .collect();
        vectors.insert(hash, vector);
    }
    drop(statement);
    // A vector whose chunk left the corpus is never read again.
    for hash in &stale {
        connection
            .execute(
                "DELETE FROM recall_embeddings WHERE hash = ?1 AND model = ?2",
                params![hash, model],
            )
            .map_err(database_error)?;
    }
    Ok(vectors)
}

fn store_vectors(
    scope_dir: &Path,
    model: &str,
    rows: &[(String, Vec<f32>)],
) -> Result<(), V2RecallError> {
    if rows.is_empty() {
        return Ok(());
    }
    let (mut connection, path) = open_cache(scope_dir)?;
    let database_error = |source| V2RecallError::Database {
        path: path.clone(),
        source,
    };
    let transaction = connection.transaction().map_err(database_error)?;
    {
        let mut statement = transaction
            .prepare(
                "INSERT OR REPLACE INTO recall_embeddings(hash, model, dims, vector)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(database_error)?;
        for (hash, vector) in rows {
            let bytes: Vec<u8> = vector
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            statement
                .execute(params![hash, model, vector.len() as i64, bytes])
                .map_err(database_error)?;
        }
    }
    transaction.commit().map_err(database_error)
}

#[cfg(test)]
#[path = "v2_recall_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "v2_recall_live_tests.rs"]
mod live_tests;
