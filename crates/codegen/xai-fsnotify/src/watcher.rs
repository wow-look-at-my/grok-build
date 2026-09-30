//! The watcher thread: it builds the debouncer, arms the watch set selection
//! asked for, then serves commands until shutdown.

use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use globset::GlobSet;
use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache, new_debouncer_opt};
use tokio::sync::mpsc;

use crate::handle::{StartProgress, WatchCommand, WatchStrategy};
use crate::install::{
    ARM_SYNC_MAX, add_subtree_watches, arm_pending_chunk, prune_subtree_watches,
    reconcile_top_level_watches,
};
use crate::merge::{RawFsEvent, merge_events};
use crate::selection::{
    MAX_TOP_LEVEL_FANOUT, event_triggers_reconcile, scan_per_dir_updates,
    select_per_dir_watch_dirs, select_top_level_watch_dirs_capped,
};
use crate::vcs::{
    GitignoreCache, find_git_dir, find_sl_dir, per_dir_git_watches, should_watch_separate_vcs_dir,
};

type Watches = Debouncer<notify::RecommendedWatcher, NoCache>;
type Ready = std::sync::mpsc::Sender<Result<(), Box<dyn Error + Send + Sync>>>;

/// What the thread needs and never changes while it runs.
pub(crate) struct Watcher {
    pub root: PathBuf,
    pub strategy: WatchStrategy,
    pub sapling: bool,
    pub debounce: Duration,
    pub custom_ignore: Arc<Option<GlobSet>>,
    pub custom_include: Arc<Option<GlobSet>>,
    pub budget: usize,
    pub events: mpsc::UnboundedSender<RawFsEvent>,
    pub commands: std::sync::mpsc::Sender<WatchCommand>,
    pub watch_count: Arc<AtomicUsize>,
    pub progress: Arc<Mutex<StartProgress>>,
}

/// The watch set, once startup arming is done.
struct Armed {
    pending: VecDeque<PathBuf>,
    watched: HashSet<PathBuf>,
    vcs: usize,
    child_mode: RecursiveMode,
    root_non_recursive: bool,
    git_dir: Option<PathBuf>,
    sl_dir: Option<PathBuf>,
}

impl Watcher {
    fn per_dir(&self) -> bool {
        self.strategy == WatchStrategy::PerDir
    }

    fn stage(&self, stage: &'static str) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.set_stage(stage);
        }
    }

    fn publish_count(&self, armed: &Armed) {
        self.watch_count
            .store(1 + armed.watched.len() + armed.vcs, Ordering::Relaxed);
    }

    pub(crate) fn run(self, cmd_rx: std::sync::mpsc::Receiver<WatchCommand>, ready_tx: Ready) {
        self.stage("watcher_thread_started");
        self.stage("creating_debouncer");
        let mut debouncer = match new_debouncer_opt::<_, notify::RecommendedWatcher, _>(
            self.debounce,
            /*tick_rate*/ None,
            self.event_sink(),
            NoCache,
            notify::Config::default().with_follow_symlinks(false),
        ) {
            Ok(debouncer) => debouncer,
            Err(e) => {
                tracing::error!("failed to create debouncer: {e:?}");
                let _ = ready_tx.send(Err(Box::new(e)));
                return;
            }
        };

        self.stage("adding_watches");
        let mut armed = match self.arm(&mut debouncer) {
            Ok(armed) => armed,
            Err(e) => {
                tracing::error!("failed to watch root: {e:?}");
                let _ = ready_tx.send(Err(Box::new(e)));
                return;
            }
        };

        self.publish_count(&armed);
        tracing::debug!(
            "fs_notify started: watching {:?} (strategy={:?}, {} dirs armed + {} pending + {} vcs watches, {}ms debounce)",
            self.root,
            self.strategy,
            armed.watched.len(),
            armed.pending.len(),
            armed.vcs,
            self.debounce.as_millis()
        );

        self.stage("signaling_ready");
        let _ = ready_tx.send(Ok(()));
        self.stage("running");
        self.serve(&mut debouncer, &mut armed, &cmd_rx);

<<<<<<< HEAD
        self.publish_count(&armed);
        tracing::debug!("fs_notify stopped");
=======
    for entry in pruning_walker(subtree_root, custom_ignore, custom_include).flatten() {
        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            if watched.contains(path) {
                continue;
            }
            if watched.len() >= budget {
                tracing::warn!(
                    "fs_notify: watch budget ({budget}) reached while adding {:?}; deeper dirs unwatched",
                    subtree_root
                );
                break;
            }
            match debouncer.watch(path, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    watched.insert(path.to_path_buf());
                }
                Err(e) => tracing::warn!("failed to watch new dir {:?}: {:?}", path, e),
            }
        } else if entry.depth() > 0 && passes_custom_globs(path, custom_ignore, custom_include) {
            backfill.push(path.to_path_buf());
            if backfill.len() >= BACKFILL_BATCH {
                flush(&mut backfill);
            }
        }
    }
    flush(&mut backfill);
}

/// Per-dir mode: drop bookkeeping (and best-effort OS watches) for a removed
/// or renamed-away directory subtree. The kernel already dropped watches on
/// deleted dirs (`IN_IGNORED`), but the explicit unwatch keeps notify's
/// path-keyed bookkeeping clean and — crucially for renames — frees the watch
/// descriptor *before* the destination path is re-watched (see
/// [`WatchCommand::Update`] ordering).
fn prune_subtree_watches(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, NoCache>,
    watched: &mut HashSet<PathBuf>,
    subtree_root: &Path,
) {
    let stale: Vec<PathBuf> = watched
        .iter()
        .filter(|p| p.starts_with(subtree_root))
        .cloned()
        .collect();
    for dir in stale {
        let _ = debouncer.unwatch(&dir); // Usually already gone; errors expected.
        watched.remove(&dir);
    }
}

pub(crate) fn start(
    watch_path: PathBuf,
    config: FsNotifyConfig,
    sapling: bool,
) -> Result<(mpsc::UnboundedReceiver<RawFsEvent>, FsNotifyHandle), crate::FsNotifyError> {
    start_with_timeout(
        watch_path,
        config,
        sapling,
        watch_strategy(),
        Duration::from_secs(WATCHER_INIT_TIMEOUT_SECS),
    )
}

/// Start with a custom timeout and explicit strategy (tests pass these
/// directly to avoid process-global env races). `sapling` is the resolved
/// kill-switch, threaded from `FsEventSource::start_on`.
pub(crate) fn start_with_timeout(
    watch_path: PathBuf,
    config: FsNotifyConfig,
    sapling: bool,
    strategy: WatchStrategy,
    init_timeout: Duration,
) -> Result<(mpsc::UnboundedReceiver<RawFsEvent>, FsNotifyHandle), crate::FsNotifyError> {
    let progress = Arc::new(Mutex::new(StartProgress::new()));
    let (tx, rx) = mpsc::unbounded_channel();
    let debounce_duration = Duration::from_millis(config.debounce_ms);
    // `.git/` (and, when `sapling`, `.sl/wlock`) pass through; the source
    // classifies internally.
    let watch_vcs = true;
    let (custom_ignore, custom_include) = build_globsets(&config.ignore_patterns);
    // Arc so the watcher thread and its debouncer callback share, not copy, them.
    let custom_ignore = Arc::new(custom_ignore);
    let custom_include = Arc::new(custom_include);

    // Canonicalize once: notify echoes event paths under the watched path, but
    // macOS FSEvents resolves symlinks, so a raw (symlinked/relative) root would
    // never match `parent() == root` and dynamic watching would silently break.
    let watch_path = dunce::canonicalize(&watch_path).unwrap_or(watch_path);

    tracing::debug!("fs_notify: starting watcher under {:?}", watch_path);

    // Channel to signal when watcher is ready
    let (ready_tx, ready_rx) =
        std::sync::mpsc::channel::<Result<(), Box<dyn std::error::Error + Send + Sync>>>();

    // Carries reconcile requests from the debouncer callback to the owning
    // thread, plus the shutdown signal from `FsNotifyHandle`.
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WatchCommand>();
    let cmd_tx_cb = cmd_tx.clone();
    let cmd_tx_for_handle = cmd_tx;

    // Live OS-watch count, shared with the handle for stats/benchmarks.
    let watch_count = Arc::new(AtomicUsize::new(0));
    let watch_count_thread = Arc::clone(&watch_count);

    // Synthetic-backfill sender for per-dir subtree adds (the debouncer
    // callback owns the primary sender).
    let backfill_tx = tx.clone();

    // `progress` records startup stages for the timeout message only: a stage that
    // is not recorded costs detail in one diagnostic, and neither the starting
    // caller nor the watcher thread should die over it. `parking_lot::Mutex` is not
    // a dependency of this crate.
    #[allow(clippy::disallowed_methods)]
    if let Ok(mut p) = progress.lock() {
        p.set_stage("spawning_watcher_thread");
>>>>>>> origin/master
    }

    /// The debouncer callback, which runs on notify's thread: it drops events
    /// for ignored paths and forwards the rest, telling this thread when the
    /// change was structural enough to alter the watch set.
    fn event_sink(&self) -> impl FnMut(DebounceEventResult) + Send + 'static {
        let root = self.root.clone();
        let strategy = self.strategy;
        let sapling = self.sapling;
        let custom_ignore = Arc::clone(&self.custom_ignore);
        let custom_include = Arc::clone(&self.custom_include);
        let events = self.events.clone();
        let commands = self.commands.clone();
        let mut gitignore = GitignoreCache::default();

<<<<<<< HEAD
        move |result: DebounceEventResult| {
            let batch = match result {
                Ok(batch) => batch,
=======
    let watcher_loop = move || {
        let update_stage = |stage: &'static str| {
            // Best-effort stage recording; see the caller above.
            #[allow(clippy::disallowed_methods)]
            if let Ok(mut p) = progress_for_thread.lock() {
                p.set_stage(stage);
            }
        };

        update_stage("watcher_thread_started");

        let mut gitignore_cache = GitignoreCache::default();
        let watch_path_cb = watch_path.clone();
        let custom_ignore_cb = Arc::clone(&custom_ignore);
        let custom_include_cb = Arc::clone(&custom_include);

        // Use NoCache to avoid walking the entire directory tree for file ID tracking.
        // This prevents multi-GB memory usage on large repos. Trade-off: rename events
        // may appear as Remove+Create pairs instead of a single Rename event.
        update_stage("creating_debouncer");
        let debouncer_result = new_debouncer_opt::<_, notify::RecommendedWatcher, _>(
            debounce_duration,
            None,
            move |result: DebounceEventResult| match result {
                Ok(events) => {
                    // Per-path (not per-event) so gitignored paths can't leak via
                    // multi-path debounced events.
                    let mut needs_reconcile = false;
                    let mut pruned: Vec<PathBuf> = Vec::new();
                    let mut added: Vec<PathBuf> = Vec::new();
                    for mut event in merge_events(events) {
                        event.paths.retain(|path| {
                            if let Some(ref include_set) = *custom_include_cb
                                && include_set.is_match(path)
                            {
                                return true;
                            }
                            if gitignore_cache.is_ignored(path, watch_vcs, sapling) {
                                return false;
                            }
                            if let Some(ref ignore_set) = *custom_ignore_cb
                                && ignore_set.is_match(path)
                            {
                                return false;
                            }
                            true
                        });
                        if event.paths.is_empty() {
                            continue;
                        }
                        // Post-retain, so ignored paths never grow the watch set.
                        match strategy {
                            WatchStrategy::Fanout => {
                                if event_triggers_reconcile(
                                    event.kind,
                                    &event.paths,
                                    &watch_path_cb,
                                ) {
                                    needs_reconcile = true;
                                }
                            }
                            WatchStrategy::PerDir => scan_per_dir_updates(
                                event.kind,
                                &event.paths,
                                &mut pruned,
                                &mut added,
                            ),
                        }
                        let _ = tx.send(event);
                    }
                    // One command per batch, not per event, to coalesce bursts.
                    if needs_reconcile {
                        let _ = cmd_tx_cb.send(WatchCommand::Reconcile);
                    }
                    if !pruned.is_empty() || !added.is_empty() {
                        let _ = cmd_tx_cb.send(WatchCommand::Update { pruned, added });
                    }
                }
>>>>>>> origin/master
                Err(errors) => {
                    for e in errors {
                        tracing::warn!("fs_notify error: {e:?}");
                    }
                    return;
                }
            };

            let mut needs_reconcile = false;
            let mut pruned: Vec<PathBuf> = Vec::new();
            let mut added: Vec<PathBuf> = Vec::new();
            for mut event in merge_events(batch) {
                event.paths.retain(|path| {
                    if let Some(include) = custom_include.as_ref()
                        && include.is_match(path)
                    {
                        return true;
                    }
                    if gitignore.is_ignored(path, /*watch_vcs*/ true, sapling) {
                        return false;
                    }
                    !custom_ignore
                        .as_ref()
                        .as_ref()
                        .is_some_and(|ignore| ignore.is_match(path))
                });
                if event.paths.is_empty() {
                    continue;
                }
                match strategy {
                    WatchStrategy::Fanout => {
                        needs_reconcile |= event_triggers_reconcile(event.kind, &event.paths, &root)
                    }
                    WatchStrategy::PerDir => {
                        scan_per_dir_updates(event.kind, &event.paths, &mut pruned, &mut added)
                    }
                }
                let _ = events.send(event);
            }

            if needs_reconcile {
                let _ = commands.send(WatchCommand::Reconcile);
            }
            if !pruned.is_empty() || !added.is_empty() {
                let _ = commands.send(WatchCommand::Update { pruned, added });
            }
        }
    }

    /// Arms the root, the directories selection chose, and the VCS metadata the
    /// root watch does not already cover. Only the root failing is fatal.
    fn arm(&self, debouncer: &mut Watches) -> Result<Armed, notify::Error> {
        let budget = self.budget;
        let initial = match self.strategy {
            WatchStrategy::PerDir => {
                let mut dirs = select_per_dir_watch_dirs(
                    &self.root,
                    &self.custom_ignore,
                    &self.custom_include,
                );
                if dirs.len() > budget {
                    tracing::warn!(
                        "fs_notify: {} non-ignored dirs exceed watch budget {budget}; shedding \
                         the deepest (raise with GROK_FSNOTIFY_MAX_WATCHES)",
                        dirs.len()
                    );
                    dirs.truncate(budget);
                }
                Some(dirs)
            }
            WatchStrategy::Fanout => select_top_level_watch_dirs_capped(
                &self.root,
                &self.custom_ignore,
                &self.custom_include,
                MAX_TOP_LEVEL_FANOUT,
            ),
        };

        let root_non_recursive = initial.is_some();
        let child_mode = if self.per_dir() {
            RecursiveMode::NonRecursive
        } else {
            RecursiveMode::Recursive
        };
        debouncer.watch(
            &self.root,
            if root_non_recursive {
                RecursiveMode::NonRecursive
            } else {
                RecursiveMode::Recursive
            },
        )?;

        // Per-dir arms the top level now and the rest in chunks, so a session
        // sees its own directory immediately however deep the tree goes.
        let mut pending = VecDeque::new();
        let mut watched = HashSet::new();
        if let Some(dirs) = initial {
            let immediate = if self.per_dir() {
                let (head, tail): (Vec<PathBuf>, Vec<PathBuf>) = dirs
                    .into_iter()
                    .partition(|d| d.parent() == Some(self.root.as_path()));
                pending = tail.into();
                head
            } else {
                dirs
            };
            for dir in immediate {
                match debouncer.watch(&dir, child_mode) {
                    Ok(()) => {
                        watched.insert(dir);
                    }
                    Err(e) => tracing::warn!("failed to watch {dir:?}: {e:?}"),
                }
            }
        }

        let mut armed = Armed {
            pending,
            watched,
            vcs: 0,
            child_mode,
            root_non_recursive,
            git_dir: find_git_dir(&self.root),
            sl_dir: self.sapling.then(|| find_sl_dir(&self.root)).flatten(),
        };
        self.arm_vcs(debouncer, &mut armed);

        if armed.pending.len() <= ARM_SYNC_MAX {
            while !armed.pending.is_empty() {
                arm_pending_chunk(
                    debouncer,
                    &mut armed.watched,
                    &mut armed.pending,
                    armed.child_mode,
                );
            }
        }
        Ok(armed)
    }

    fn arm_vcs(&self, debouncer: &mut Watches, armed: &mut Armed) {
        let root_non_recursive = armed.root_non_recursive;
        let git_dir = armed.git_dir.clone();
        let sl_dir = armed.sl_dir.clone();
        let separate = |dir: &PathBuf| {
            should_watch_separate_vcs_dir(root_non_recursive, dir.as_path(), &self.root)
        };

        if let Some(git_dir) = git_dir.as_ref().filter(|dir| separate(dir)) {
            let watches = if self.per_dir() {
                per_dir_git_watches(git_dir)
            } else {
                vec![(git_dir.clone(), RecursiveMode::Recursive)]
            };
            for (path, mode) in watches {
                match debouncer.watch(&path, mode) {
                    Ok(()) => armed.vcs += 1,
                    Err(e) => tracing::warn!("failed to watch git path {path:?}: {e:?}"),
                }
            }
            tracing::debug!("fs_notify: watching git dir {git_dir:?}");
        }

        if let Some(sl_dir) = sl_dir.as_ref().filter(|dir| separate(dir)) {
            match debouncer.watch(sl_dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    armed.vcs += 1;
                    tracing::debug!("fs_notify: watching sl dir {sl_dir:?}");
                }
                Err(e) => tracing::warn!("failed to watch sl dir {sl_dir:?}: {e:?}"),
            }
        }
    }

    /// Serves commands, arming any backlog between them so a large tree keeps
    /// making progress without delaying the ones that arrive.
    fn serve(
        &self,
        debouncer: &mut Watches,
        armed: &mut Armed,
        cmd_rx: &std::sync::mpsc::Receiver<WatchCommand>,
    ) {
        loop {
            loop {
                match cmd_rx.try_recv() {
                    Ok(cmd) => {
                        if self.apply(cmd, debouncer, armed).is_break() {
                            return;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                }
            }

            if armed.pending.is_empty() {
                self.publish_count(armed);
                match cmd_rx.recv() {
                    Ok(cmd) => {
                        if self.apply(cmd, debouncer, armed).is_break() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            } else {
                arm_pending_chunk(
                    debouncer,
                    &mut armed.watched,
                    &mut armed.pending,
                    armed.child_mode,
                );
                self.publish_count(armed);
            }
        }
    }

    fn apply(
        &self,
        cmd: WatchCommand,
        debouncer: &mut Watches,
        armed: &mut Armed,
    ) -> ControlFlow<()> {
        match cmd {
            WatchCommand::Reconcile => {
                if !self.per_dir() && armed.root_non_recursive {
                    reconcile_top_level_watches(
                        debouncer,
                        &mut armed.watched,
                        &self.root,
                        &self.custom_ignore,
                        &self.custom_include,
                    );
                }
            }
            WatchCommand::Update { pruned, added } if self.per_dir() => {
                // Prune-before-add ordering: see [`WatchCommand::Update`].
                for path in &pruned {
                    if armed.watched.contains(path) {
                        prune_subtree_watches(debouncer, &mut armed.watched, path);
                    }
                }
                for path in &added {
                    if self.is_root_or_vcs_dir(path, armed) {
                        continue;
                    }
                    add_subtree_watches(
                        debouncer,
                        &mut armed.watched,
                        &self.root,
                        path,
                        &self.custom_ignore,
                        &self.custom_include,
                        self.budget,
                        &self.events,
                    );
                }
            }
            WatchCommand::Update { .. } => {}
            WatchCommand::Shutdown => return ControlFlow::Break(()),
        }
<<<<<<< HEAD
        ControlFlow::Continue(())
=======
    };
    let thread = std::thread::Builder::new()
        .name("fsnotify-watcher".into())
        .spawn(watcher_loop)
        .map_err(|e| crate::FsNotifyError::WatcherStart(Box::new(e)))?;

    // Wait for watcher to be ready (with timeout)
    #[allow(clippy::disallowed_methods)] // best-effort stage recording
    if let Ok(mut p) = progress.lock() {
        p.set_stage("waiting_for_ready");
    }
    match ready_rx.recv_timeout(init_timeout) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(crate::FsNotifyError::WatcherStart(e)),
        Err(_) => {
            // A poison answers the same tuple an unreadable progress would: the
            // stage names are diagnostics, never the watcher's state.
            #[allow(clippy::disallowed_methods)]
            let (stage, stage_elapsed, total_elapsed, timeline) =
                progress.lock().map(|p| p.snapshot()).unwrap_or((
                    "unknown",
                    Duration::from_secs(0),
                    Duration::from_secs(0),
                    Vec::new(),
                ));
            tracing::debug!(
                "watcher start timed out ({}s): stage={}, stage_elapsed={:?}, total_elapsed={:?}, timeline={:?}",
                init_timeout.as_secs(),
                stage,
                stage_elapsed,
                total_elapsed,
                timeline
            );
            // No `FsNotifyHandle` owns the thread on this path, so queue a
            // Shutdown: when the slow setup finishes and the thread reaches its
            // recv loop, it self-terminates and releases its watches instead of
            // leaking (the callback holds the other sender, so it never
            // disconnects on its own).
            let _ = cmd_tx_for_handle.send(WatchCommand::Shutdown);
            return Err(crate::FsNotifyError::Timeout);
        }
>>>>>>> origin/master
    }

    /// The root and the VCS directories are armed once at startup and never
    /// through the incremental path.
    fn is_root_or_vcs_dir(&self, path: &Path, armed: &Armed) -> bool {
        path == self.root
            || armed
                .git_dir
                .as_deref()
                .is_some_and(|dir| path.starts_with(dir))
            || armed
                .sl_dir
                .as_deref()
                .is_some_and(|dir| path.starts_with(dir))
    }
}

// watcher_tests.rs was written against the pre-split module. Import the
// relocated items so those tests compile unchanged via `use super::*`.
#[cfg(test)]
#[allow(unused_imports)]
use crate::event::FsEventKind;
#[cfg(test)]
#[allow(unused_imports)]
use crate::handle::{
    DEBOUNCE_MS, FsNotifyConfig, FsNotifyHandle, start, start_with_timeout, watch_strategy,
};
#[cfg(test)]
#[allow(unused_imports)]
use crate::merge::map_event_kind;
#[cfg(test)]
#[allow(unused_imports)]
use crate::selection::{
    build_globsets, diff_watches, is_top_level_child, select_top_level_watch_dirs,
};
#[cfg(test)]
#[allow(unused_imports)]
use crate::vcs::{is_git_path_for_watcher, is_sl_path_for_watcher, sapling_enabled};
#[cfg(test)]
#[allow(unused_imports)]
use notify::event::EventKind;
#[cfg(test)]
#[allow(unused_imports)]
use notify_debouncer_full::DebouncedEvent;

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
