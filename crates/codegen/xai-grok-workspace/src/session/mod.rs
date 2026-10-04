pub(crate) mod checkpoint;
pub(crate) mod checkpoint_store;
pub mod file_state;
pub mod git;
pub(crate) mod git_gate;
pub mod jj;
pub(crate) mod swap_policy;
pub mod tool_config;
use crate::capability::CapabilityMode;
use crate::config::{MemoryConfig, SessionContextFactory};
use crate::file_system::{AsyncFsWrapper, LocalFs};
use crate::hub::{HubConfig, HubHandle};
use crate::session::file_state::FileStateTracker;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use xai_computer_hub_mcp_adapter::McpBridgeHandle;
use xai_grok_mcp::servers::McpState;
use xai_grok_tools::notification::AcknowledgedToolNotification;
use xai_grok_tools::notification::types::ToolNotificationHandle;
use xai_grok_tools::registry::types::{FinalizedToolset, ToolConfig, ToolServerConfig};
use xai_hunk_tracker::HunkTrackerHandle;
use xai_tool_protocol::ToolId;
use xai_tool_runtime::WorkspaceViewerContext;
/// Minimal result types for git error reporting (duplicated from shell session/result).
pub mod result {
    use serde::Serialize;
    #[derive(Debug, Serialize)]
    pub struct ExtMethodError {
        pub code: i32,
        pub message: String,
        pub data: Option<serde_json::Value>,
    }
    impl ExtMethodError {
        pub fn with_data(code: i32, msg: String, data: impl Serialize) -> Self {
            Self {
                code,
                message: msg,
                data: serde_json::to_value(data).ok(),
            }
        }
    }
    #[derive(Debug)]
    pub struct ExtMethodResult<T> {
        pub result: Option<T>,
        pub error: Option<serde_json::Value>,
    }
}
/// One MCP server running for a session.
pub(crate) struct SessionMcpServer {
    pub(crate) bridge: McpBridgeHandle,
    /// The ids this server contributed to the session's advertised tool set, i.e. Dropping the server unregisters exactly these.
    pub(crate) tool_ids: Vec<ToolId>,
    /// Fixed for the server's life from the bind config it started under: re-claims.
    pub(crate) tier: crate::mcp_claim::McpServerTier,
}
/// The MCP servers a session is running.
pub(crate) struct ActiveMcp {
    /// Config server name → live server.
    pub(crate) servers: HashMap<String, SessionMcpServer>,
    /// Servers stopped while they stay configured: a reload leaves them stopped.
    pub(crate) stopped: HashSet<String>,
}
/// Whether a session takes part in the workspace's configured MCP set.
pub(crate) enum WorkspaceMcpBinding {
    Uninitialized,
    Active(ActiveMcp),
    Closed,
}
impl WorkspaceMcpBinding {
    /// The session's servers, or `None` if it has not joined the set.
    pub(crate) fn active(&self) -> Option<&ActiveMcp> {
        match self {
            Self::Active(active) => Some(active),
            Self::Uninitialized | Self::Closed => None,
        }
    }
    /// Mutable [`Self::active`]. Does **not** enrol a session that has not
    /// joined — pushing servers into an `rpc_only` bind would hand it tools
    /// it opted out of.
    pub(crate) fn active_mut(&mut self) -> Option<&mut ActiveMcp> {
        match self {
            Self::Active(active) => Some(active),
            Self::Uninitialized | Self::Closed => None,
        }
    }
    /// Enrol this session in the configured set, or return its existing
    /// servers. The only accessor that promotes, and `None` only once torn
    /// down — a reload must never resurrect a session the hub has ended.
    pub(crate) fn join(&mut self) -> Option<&mut ActiveMcp> {
        if matches!(self, Self::Uninitialized) {
            *self = Self::Active(ActiveMcp {
                servers: HashMap::new(),
                stopped: HashSet::new(),
            });
        }
        self.active_mut()
    }
}
/// How one of a session's MCP servers fared once its start settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerOutcome {
    Connected,
    Failed,
}
/// Per-session state held in [`WorkspaceShared::sessions`].
///
/// The `effective_tool_config` baseline and the resolved `toolset` are kept under a single `RwLock` so a hot reload swaps both atomically.
pub struct WorkspaceSession {
    pub(crate) session_id: String,
    pub(crate) cwd: PathBuf,
    pub(crate) session_env: Arc<HashMap<String, String>>,
    pub(crate) capability_mode: CapabilityMode,
    pub(crate) depth: u32,
    pub(crate) fork_budget: u32,
    pub(crate) hunk_tracker: HunkTrackerHandle,
    /// Cancel token for the workspace-spawned hunk tracker. [`Self::cancel_hunk_tracker`] fires it on session teardown.
    pub(crate) hunk_tracker_cancel: Option<tokio_util::sync::CancellationToken>,
    pub(crate) file_state_tracker: Arc<FileStateTracker>,
    /// Per-turn hunk deltas keyed by `prompt_index`.
    pub(crate) hunk_checkpoints:
        Arc<tokio::sync::Mutex<HashMap<usize, xai_hunk_tracker::HunkTurnDelta>>>,
    /// Git domain of the per-prompt rewind checkpoints (HEAD and the staged set).
    pub(crate) git_checkpoints: crate::session::git::GitCheckpointStore,
    /// Disk-backed durability mirror for finalized checkpoints, fronted by an in-memory cache. Only a mirror: restore stays in-process.
    pub(crate) checkpoint_store: crate::session::checkpoint_store::CheckpointStore,
    pub(crate) async_fs: AsyncFsWrapper,
    inner: RwLock<WorkspaceSessionInner>,
    /// Per-session lock that serialises `update_tool_config` calls.
    pub(crate) update_lock: tokio::sync::Mutex<()>,
    /// Canonical MCP client/configuration runtime state.
    pub(crate) mcp_state: Arc<tokio::sync::Mutex<McpState>>,
    /// Workspace hub publication state for MCP bridges and registered aliases.
    pub(crate) mcp_binding: tokio::sync::Mutex<WorkspaceMcpBinding>,
    /// Cancels the session's in-flight MCP server starts.
    pub(crate) mcp_cancel: parking_lot::Mutex<tokio_util::sync::CancellationToken>,
    /// MCP life counter, only ever bumped under `mcp_binding`: an enrolment that TRANSITIONS the binding to Active starts a life.
    pub(crate) mcp_epoch: std::sync::atomic::AtomicU64,
    /// Accepted-bind counter, bumped under `mcp_binding` by every bind the resolver accepts.
    pub(crate) mcp_bind_generation: std::sync::atomic::AtomicU64,
    /// The native tool ids the session's last bind advertised (including the RPC handler).
    pub(crate) mcp_native_tool_ids: parking_lot::Mutex<std::collections::HashSet<ToolId>>,
    /// Per-user feature-flag bag resolved at session-bind time, frozen for the session lifetime.
    pub(crate) viewer_ctx: Option<WorkspaceViewerContext>,
    /// Auto-approve (YOLO) state. Seeded from `session.bind` metadata, refreshed by each before-turn hook.
    pub(crate) yolo_mode: std::sync::atomic::AtomicBool,
    /// The hub approval gate's per-session state (ceiling, folder grants).
    pub(crate) approval: crate::permission::SessionApproval,
    /// Session-lifetime terminal backend (background-task registry and persistent shell).
    terminal_backend: crate::config::SessionTerminalBackend,
    /// Canonical JSON of the explicit `session.bind` toolset this session was created (or last rebound) with.
    bind_tool_config_fingerprint: std::sync::Mutex<Option<serde_json::Value>>,
    /// The last snapshot-driven rebuild failed and kept a stale toolset; cleared by any successful install.
    stale_resolve: std::sync::atomic::AtomicBool,
    /// Whether this session forwards `BackgroundTaskCompleted` system notifications.
    #[allow(dead_code)]
    system_notifications: bool,
    /// Per-session notification sender, re-applied across toolset re-resolves.
    system_notify_handle: Option<ToolNotificationHandle>,
    /// Receiver paired with `system_notify_handle`, taken once by the forwarder.
    #[allow(dead_code)]
    pending_notif_rx: tokio::sync::Mutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<AcknowledgedToolNotification>>,
    >,
    /// Spawned system-notify producers (forwarder, preview-state watcher).
    system_notify_producers: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Path mapping between the guest and model views. Set once at bind from `session_root`.
    path_virtualization: OnceLock<crate::path_virtualization::PathVirtualization>,
    /// Rewritten bind cwd installed on rebind when path virt turns on after the session was first created without `session_root`.
    cwd_override: OnceLock<PathBuf>,
    /// In-progress `workspace.client_fs_write_file` uploads bound to this session.
    staged_uploads: crate::file_system::client_fs::StagedUploads,
}
struct WorkspaceSessionInner {
    effective_tool_config: Arc<ToolServerConfig>,
    toolset: Arc<FinalizedToolset>,
}
impl std::fmt::Debug for WorkspaceSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSession")
            .field("session_id", &self.session_id)
            .field("cwd", &self.cwd())
            .field("capability_mode", &self.capability_mode)
            .field("depth", &self.depth)
            .field("fork_budget", &self.fork_budget)
            .finish_non_exhaustive()
    }
}
/// Copy pre-virt working-tree files into `new` so accepting a hunk after the remount still sees content written under the old cwd.
/// Skips the dest subtree and sibling session trees (`/workspace/<other-uuid>`).
fn relocate_pre_virt_files(old: &Path, new: &Path) {
    if std::fs::create_dir_all(new).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(old) else {
        return;
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if src == new {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&src) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() && uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok() {
            continue;
        }
        let dest = new.join(entry.file_name());
        if dest.symlink_metadata().is_ok() {
            continue;
        }
        if meta.is_dir() {
            let _ = copy_dir_all(&src, &dest);
        } else {
            let _ = std::fs::copy(&src, &dest);
        }
    }
}
fn copy_dir_all(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&from) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let to = dest.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
impl WorkspaceSession {
    pub(crate) fn new(
        session_id: String,
        cwd: PathBuf,
        session_env: Arc<HashMap<String, String>>,
        capability_mode: CapabilityMode,
        depth: u32,
        fork_budget: u32,
        effective_tool_config: Arc<ToolServerConfig>,
        toolset: Arc<FinalizedToolset>,
        terminal_backend: crate::config::SessionTerminalBackend,
        hunk_tracker: HunkTrackerHandle,
        hunk_tracker_cancel: Option<tokio_util::sync::CancellationToken>,
        viewer_ctx: Option<WorkspaceViewerContext>,
        #[allow(dead_code)] system_notifications: bool,
        system_notify_channel: Option<(
            ToolNotificationHandle,
            tokio::sync::mpsc::UnboundedReceiver<AcknowledgedToolNotification>,
        )>,
    ) -> Self {
        let (system_notify_handle, pending_notif_rx) = match system_notify_channel {
            Some((handle, rx)) => (Some(handle), Some(rx)),
            None => (None, None),
        };
        let async_fs = AsyncFsWrapper::new(Arc::new(LocalFs::new(cwd.clone())));
        let file_state_tracker = Arc::new(FileStateTracker::new());
        let checkpoint_store =
            crate::session::checkpoint_store::CheckpointStore::new(&cwd, &session_id);
        Self {
            session_id,
            cwd,
            session_env,
            capability_mode,
            depth,
            fork_budget,
            hunk_tracker,
            hunk_tracker_cancel,
            file_state_tracker,
            hunk_checkpoints: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            git_checkpoints: crate::session::git::GitCheckpointStore::new(),
            checkpoint_store,
            async_fs,
            inner: RwLock::new(WorkspaceSessionInner {
                effective_tool_config,
                toolset,
            }),
            terminal_backend,
            update_lock: tokio::sync::Mutex::new(()),
            bind_tool_config_fingerprint: std::sync::Mutex::new(None),
            stale_resolve: std::sync::atomic::AtomicBool::new(false),
            mcp_state: Arc::new(tokio::sync::Mutex::new(McpState::new(vec![]))),
            mcp_binding: tokio::sync::Mutex::new(WorkspaceMcpBinding::Uninitialized),
            mcp_cancel: parking_lot::Mutex::new(tokio_util::sync::CancellationToken::new()),
            mcp_epoch: std::sync::atomic::AtomicU64::new(0),
            mcp_bind_generation: std::sync::atomic::AtomicU64::new(0),
            mcp_native_tool_ids: parking_lot::Mutex::new(std::collections::HashSet::new()),
            viewer_ctx,
            yolo_mode: std::sync::atomic::AtomicBool::new(false),
            approval: crate::permission::SessionApproval::default(),
            system_notifications,
            system_notify_handle,
            #[allow(dead_code)]
            pending_notif_rx: tokio::sync::Mutex::new(pending_notif_rx),
            system_notify_producers: std::sync::Mutex::new(Vec::new()),
            path_virtualization: OnceLock::new(),
            cwd_override: OnceLock::new(),
            staged_uploads: Default::default(),
        }
    }
    /// Staged `client_fs_write_file` uploads owned by this session.
    pub(crate) fn staged_uploads(&self) -> &crate::file_system::client_fs::StagedUploads {
        &self.staged_uploads
    }
    pub(crate) fn set_path_virtualization(
        &self,
        mapping: crate::path_virtualization::PathVirtualization,
    ) {
        let _ = self.path_virtualization.set(mapping);
    }
    /// Install a rewritten bind cwd on a session that already exists. First-bind `create_session` already constructed `cwd`; rebind only fills the `path_virtualization` `OnceLock`.
    pub(crate) async fn set_cwd_for_virtualization(&self, cwd: PathBuf) -> Result<(), String> {
        if self.cwd() == cwd.as_path() {
            return Ok(());
        }
        let old = self.cwd().to_path_buf();
        if old != cwd {
            let dest = cwd.clone();
            let _ = tokio::task::spawn_blocking(move || relocate_pre_virt_files(&old, &dest)).await;
        }
        if !self.checkpoint_store.remount(&cwd, &self.session_id).await {
            return Err("checkpoint remount failed".into());
        }
        let _ = self.cwd_override.set(cwd.clone());
        self.async_fs.remount_root(cwd.clone());
        self.hunk_tracker.set_working_dir(cwd.clone()).await;
        let toolset = self.toolset();
        let mut res = toolset.resources.lock().await;
        res.insert(xai_grok_tools::types::resources::Cwd(cwd));
        Ok(())
    }
    pub(crate) fn path_virtualization(
        &self,
    ) -> Option<&crate::path_virtualization::PathVirtualization> {
        self.path_virtualization.get()
    }
    /// Whether this session opted into `BackgroundTaskCompleted` system notifications.
    #[allow(dead_code)]
    pub(crate) fn system_notifications(&self) -> bool {
        self.system_notifications
    }
    /// The per-session notification sender, re-applied on every toolset re-resolve so notifications keep flowing to the forwarder's channel.
    pub(crate) fn system_notify_handle(&self) -> Option<ToolNotificationHandle> {
        self.system_notify_handle.clone()
    }
    /// Hand the notification receiver to the forwarder. Works once.
    #[allow(dead_code)]
    pub(crate) async fn take_pending_notif_rx(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<AcknowledgedToolNotification>> {
        self.pending_notif_rx.lock().await.take()
    }
    /// True once a producer set has been tracked; finalize spawns at most one (a re-finalize must not abort a forwarder it cannot respawn).
    #[allow(dead_code)]
    pub(crate) fn has_system_notify_producers(&self) -> bool {
        !self
            .system_notify_producers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
    /// Track the spawned system-notify producers, aborting any previous set.
    #[allow(dead_code)]
    pub(crate) fn set_system_notify_producers(&self, handles: Vec<tokio::task::JoinHandle<()>>) {
        let mut guard = self
            .system_notify_producers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for old in guard.drain(..) {
            old.abort();
        }
        *guard = handles;
    }
    /// Abort every tracked system-notify producer on teardown.
    pub(crate) fn abort_system_notify_producers(&self) {
        for handle in self
            .system_notify_producers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            handle.abort();
        }
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// The server's settled start outcome, or `None` while it is still starting. Never waits: a
    /// status probe reads past a start that holds the lock and reports it as unsettled.
    pub fn mcp_server_outcome(&self, name: &str) -> Option<McpServerOutcome> {
        let state = self.mcp_state.try_lock().ok()?;
        if state.owned_clients.contains_key(name) {
            Some(McpServerOutcome::Connected)
        } else if state.init_failed.contains_key(name) {
            Some(McpServerOutcome::Failed)
        } else {
            None
        }
    }
    /// Settles `name` as a start would, for host-crate tests that read the outcome without a server.
    #[doc(hidden)]
    pub async fn settle_mcp_server_for_test(&self, name: &str, outcome: McpServerOutcome) {
        let mut state = self.mcp_state.lock().await;
        match outcome {
            McpServerOutcome::Connected => {
                state.owned_clients.insert(
                    name.to_owned(),
                    Arc::new(xai_grok_mcp::servers::McpClient::stub(name)),
                );
            }
            McpServerOutcome::Failed => state.record_init_failure(name, false, None),
        }
    }
    pub fn cwd(&self) -> &Path {
        self.cwd_override.get().map_or(&self.cwd, PathBuf::as_path)
    }
    pub fn session_env(&self) -> &Arc<HashMap<String, String>> {
        &self.session_env
    }
    pub fn capability_mode(&self) -> CapabilityMode {
        self.capability_mode
    }
    pub fn depth(&self) -> u32 {
        self.depth
    }
    pub fn fork_budget(&self) -> u32 {
        self.fork_budget
    }
    pub fn hunk_tracker(&self) -> &HunkTrackerHandle {
        &self.hunk_tracker
    }
    /// Per-user feature-flag bag resolved at session-bind time.
    pub fn viewer_ctx(&self) -> Option<&WorkspaceViewerContext> {
        self.viewer_ctx.as_ref()
    }
    pub fn yolo_mode(&self) -> bool {
        self.yolo_mode.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set_yolo_mode(&self, enabled: bool) {
        self.yolo_mode
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn file_state_tracker(&self) -> &Arc<FileStateTracker> {
        &self.file_state_tracker
    }
    /// Git domain of the per-prompt rewind checkpoints.
    pub fn git_checkpoints(&self) -> &crate::session::git::GitCheckpointStore {
        &self.git_checkpoints
    }
    pub fn async_fs(&self) -> &AsyncFsWrapper {
        &self.async_fs
    }
    /// The session-lifetime terminal backend, injected into every toolset re-resolve so background tasks and shell state survive swaps.
    pub(crate) fn terminal_backend(
        &self,
    ) -> &Arc<dyn xai_grok_tools::computer::types::TerminalBackend> {
        self.terminal_backend.backend()
    }
    /// Explicitly shut the session's terminal backend down (kills all of its
    /// child process groups and stops its actor).
    pub(crate) fn shutdown_terminal_backend(&self) {
        self.terminal_backend.shutdown();
    }
    /// No-op when the browser tools are compiled out.
    pub(crate) fn shutdown_browser_service(&self) {}
    /// Cancel the workspace-spawned hunk-tracker actor, if this session owns
    /// one. The actor pins file contents in `file_states`.
    pub(crate) fn cancel_hunk_tracker(&self) {
        if let Some(token) = &self.hunk_tracker_cancel {
            token.cancel();
        }
    }
    /// Return the current resolved toolset (snapshot).
    pub fn toolset(&self) -> Arc<FinalizedToolset> {
        self.inner.read().toolset.clone()
    }
    /// Whether the current toolset's `Terminal` resource is the session-owned backend. Rebuild paths must skip such sessions: finalizing around [`Self::terminal_backend`] would detach tools from the shell's live task table.
    pub(crate) async fn toolset_terminal_is_session_owned(&self) -> bool {
        let toolset = self.toolset();
        let res = toolset.resources.lock().await;
        match res.get::<xai_grok_tools::types::resources::Terminal>() {
            Some(t) => Arc::ptr_eq(&t.0, self.terminal_backend()),
            None => true,
        }
    }
    /// Return the current effective tool config baseline.
    pub fn effective_tool_config(&self) -> Arc<ToolServerConfig> {
        self.inner.read().effective_tool_config.clone()
    }
    /// Whether `fingerprint` matches the explicit bind toolset this session was created (or last rebound) with.
    /// `None` means a default resolution.
    #[cfg(test)]
    pub(crate) fn bind_tool_config_matches(&self, fingerprint: Option<&serde_json::Value>) -> bool {
        let guard = self
            .bind_tool_config_fingerprint
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.as_ref() == fingerprint
    }
    /// Whether the last snapshot-driven rebuild failed and left the live toolset stale w.r.t. the current MCP/hub snapshots.
    pub(crate) fn stale_resolve(&self) -> bool {
        self.stale_resolve
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Mark the live toolset stale: a snapshot-driven rebuild failed and the previous toolset was kept.
    pub(crate) fn mark_stale_resolve(&self) {
        self.stale_resolve
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    /// Clear the stale marker: a freshly resolved toolset was installed.
    pub(crate) fn clear_stale_resolve(&self) {
        self.stale_resolve
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
    /// Record the explicit bind toolset (or `None` for a default resolution)
    /// this session's toolset was resolved from.
    pub(crate) fn set_bind_tool_config_fingerprint(&self, fingerprint: Option<serde_json::Value>) {
        *self
            .bind_tool_config_fingerprint
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = fingerprint;
    }
    /// [`Self::set_bind_tool_config_fingerprint`], but only when no
    /// fingerprint was recorded yet. A concurrent rebind can race between
    /// session insertion and this call, swap in its own toolset, and record
    /// its fingerprint under the lock.
    pub(crate) fn set_bind_tool_config_fingerprint_if_unset(
        &self,
        fingerprint: Option<serde_json::Value>,
    ) {
        let mut guard = self
            .bind_tool_config_fingerprint
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = fingerprint;
        }
    }
    /// Replace both the baseline config and the resolved toolset atomically.
    /// TOOL-STATE CAVEAT: the outgoing toolset is not flushed here.
    pub(crate) fn replace(
        &self,
        new_effective_tool_config: Arc<ToolServerConfig>,
        new_toolset: Arc<FinalizedToolset>,
    ) {
        let mut w = self.inner.write();
        w.effective_tool_config = new_effective_tool_config;
        w.toolset = new_toolset;
    }
    /// [`Self::replace`], but first carries the session's `BrowserServiceHandle` from the old toolset into the new one.
    /// The browser service seeded by `finalize_session_setup` must be carried forward or the session's live browser state is lost.
    /// Without the optional browser backend there is no browser service to carry; only the terminal-orphan diagnostic runs before the swap.
    pub(crate) async fn replace_carrying_browser_service(
        &self,
        new_effective_tool_config: Arc<ToolServerConfig>,
        new_toolset: Arc<FinalizedToolset>,
    ) {
        let old_toolset = self.toolset();
        let old_terminal = {
            let res = old_toolset.resources.lock().await;
            res.get::<xai_grok_tools::types::resources::Terminal>()
                .map(|t| t.0.clone())
        };
        if let Some(old_terminal) = old_terminal
            && !Arc::ptr_eq(&old_terminal, self.terminal_backend())
        {
            crate::handle::WORKSPACE_TERMINAL_BACKEND_ORPHANED_TOTAL
                .with_label_values(&["swap"])
                .inc();
            tracing::error!(
                session_id = %self.session_id,
                "toolset swap: outgoing toolset's terminal backend is not the \
                 session-owned one — its background tasks die with the old toolset"
            );
        }
        self.replace(new_effective_tool_config, new_toolset);
    }
}
/// Sink for delivering a workspace-originated ext-notification (method and params JSON) to the client.
pub type ClientExtSink = std::sync::Arc<dyn Fn(String, serde_json::Value) + Send + Sync>;
/// Workspace-wide shared state.
pub struct WorkspaceShared {
    pub(crate) default_tool_config: ToolServerConfig,
    /// Require an explicit toolset on every `session.bind`; see [`crate::config::WorkspaceConfig::require_explicit_toolset`].
    pub(crate) require_explicit_toolset: bool,
    /// See [`crate::config::WorkspaceConfig::confine_fs_to_workspace_root`].
    pub(crate) confine_fs_to_workspace_root: bool,
    /// See [`crate::config::WorkspaceConfig::tool_approval`].
    pub(crate) tool_approval: crate::permission::ToolApprovalGate,
    /// Which host runs this server (`WorkspaceConfig::host_kind`).
    pub(crate) host_kind: crate::host_kind::WorkspaceHostKind,
    /// Workspace root directory. Independent of any session; stored here so it survives session creation/deletion.
    pub(crate) root_cwd: std::path::PathBuf,
    pub(crate) sessions: RwLock<HashMap<String, Arc<WorkspaceSession>>>,
    pub(crate) session_factory: Arc<dyn SessionContextFactory>,
    /// `Some` iff every admitted session binds a configured set of MCP servers.
    pub(crate) bind_mcp: Option<parking_lot::RwLock<crate::config::BindMcpConfig>>,
    pub(crate) mcp_tools_snapshot: arc_swap::ArcSwap<Vec<ToolConfig>>,
    pub(crate) events: tokio::sync::broadcast::Sender<xai_grok_workspace_types::WorkspaceEvent>,
    pub(crate) respect_gitignore: bool,
    pub(crate) memory_config: Option<MemoryConfig>,
    pub(crate) hook_registry: Arc<parking_lot::RwLock<xai_grok_hooks::discovery::HookRegistry>>,
    pub(crate) hook_load_errors: Vec<xai_grok_hooks::error::HookError>,
    /// Skill discovery configuration (extra paths, ignore prefixes).
    pub(crate) skills_config: crate::discovery::SkillsConfig,
    /// Plugin discovery configuration (CLI dirs, config paths, disabled/enabled lists).
    pub(crate) plugin_discovery_config: crate::discovery::PluginDiscoveryConfig,
    /// Live server connection handle.
    pub(crate) hub_handle: tokio::sync::Mutex<Option<HubHandle>>,
    pub(crate) queue_stats_sampler:
        parking_lot::Mutex<Option<crate::upload::QueueStatsSamplerGuard>>,
    /// Remote-origin tool configs (consumer direction), updated by the notification listener.
    pub(crate) hub_tools_snapshot: arc_swap::ArcSwap<Vec<ToolConfig>>,
    /// Server config stashed at construction time for deferred connect.
    pub(crate) hub_config: Option<HubConfig>,
    /// Auth provider for xAI service calls.
    pub(crate) auth_provider: Option<xai_computer_hub_sdk::SharedAuthProvider>,
    /// Connection-level sink feeding the `ActivityTracker` (drained by
    /// `run_activity_feed`); not a network egress.
    pub(crate) activity_notify_handle:
        arc_swap::ArcSwap<Option<xai_grok_tools::notification::types::ToolNotificationHandle>>,
    /// Sink for workspace-originated ext-notifications to the client (e.g. `x.ai/search/fuzzy/status`).
    pub(crate) client_ext_sink: arc_swap::ArcSwap<Option<ClientExtSink>>,
    pub(crate) local_registry: xai_computer_hub_sdk::LocalRegistry,
    pub(crate) activity_tracker: std::sync::Arc<crate::activity::ActivityTracker>,
    /// True after the scheduler-liveness poll task started; reconnects must not start a second one.
    pub(crate) scheduler_poll_started: std::sync::atomic::AtomicBool,
    /// Runtime-tunable timing/threshold config for the tool server. Read by the status publisher task and at shutdown.
    pub(crate) status_config: crate::status_config::StatusConfig,
    /// Opaque metadata for the tool server registration, forwarded verbatim to the server.
    pub(crate) server_metadata: Option<serde_json::Value>,
    /// Owner identity, captured at construction; stamps `workspace_environment.json` and attributes uploads.
    pub(crate) identity: crate::upload::environment::WorkspaceIdentity,
    /// Workspace-level fuzzy search manager.
    pub(crate) fuzzy_searches:
        std::sync::Arc<tokio::sync::Mutex<crate::file_system::FuzzySearchManager>>,
    pub(crate) lsp: Option<std::sync::Arc<dyn xai_grok_tools::implementations::lsp::LspBackend>>,
    pub(crate) codebase_indexes:
        std::sync::Arc<parking_lot::Mutex<crate::file_system::CodebaseIndexManager>>,
    /// Finalize the FS rewind checkpoint on non-`Completed` turn-end outcomes (from `GROK_WORKSPACE_REWIND_ALL_OUTCOMES`, default off).
    pub(crate) workspace_rewind_all_outcomes: bool,
    /// Resolved `$GROK_WORKSPACE_HOME`, the workspace-owned on-disk state root (`<grok_home>/workspace` by default).
    pub(crate) workspace_home: std::path::PathBuf,
    pub(crate) upload_queue: Option<std::sync::Arc<xai_file_utils::queue::UploadQueue>>,
    /// Whether collection is disabled (opt-out, or the fail-closed default).
    pub(crate) data_collection_disabled: bool,
    /// Whether per-session `events.jsonl` recording is enabled (`GROK_WORKSPACE_EVENTS_ENABLED=true`).
    pub(crate) events_enabled: bool,
    /// Whether per-session `workspace_tool_definitions.json` emission is enabled (`GROK_WORKSPACE_TOOL_DEFS_ENABLED=true`).
    pub(crate) tool_defs_enabled: bool,
    /// Maps `session_id` to the last `ToolsChanged` re-emit `Instant`, debouncing re-emits per session.
    pub(crate) tool_defs_last_emit: dashmap::DashMap<String, std::time::Instant>,
    /// Per-session `events.jsonl` writers, keyed by `session_id`.
    pub(crate) session_event_writers:
        Arc<dashmap::DashMap<String, xai_grok_session_events::EventWriter>>,
    /// In-flight before-turn enqueue tasks, keyed by `(session_id, turn)`.
    /// Stored by `on_before_turn`; evicted on every turn-end path.
    pub(crate) inflight_enqueues: dashmap::DashMap<
        (String, u64),
        tokio::task::JoinHandle<xai_file_utils::queue::EnqueueOutcome>,
    >,
    /// Artifact-producer tasks, awaited by the drain and counted by the status publisher.
    pub(crate) producer_tasks: tokio_util::task::TaskTracker,
    /// Bind-time probe-then-mount hook. Default no-op until a command is set.
    pub(crate) bind_mount_hook: arc_swap::ArcSwap<crate::path_virtualization::BindMountHook>,
    /// Memo of sha256 by `(path, size, mtime_ms)` for the client-facing `workspace.client_fs_*` ops.
    #[cfg(test)]
    pub(crate) post_resolve_test_hook: parking_lot::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub(crate) client_fs_hash_memo: crate::file_system::client_fs::FileHashMemo,
}
impl WorkspaceShared {
    /// Workspace root directory.
    pub fn root_cwd(&self) -> &std::path::Path {
        &self.root_cwd
    }
    /// Resolved `$GROK_WORKSPACE_HOME`, the workspace-owned on-disk state root.
    pub fn workspace_home(&self) -> &std::path::Path {
        &self.workspace_home
    }
    /// The durable upload queue used for archives.
    /// `None` in tests and local mode; see [`WorkspaceShared::upload_queue`].
    pub fn upload_queue(&self) -> Option<&std::sync::Arc<xai_file_utils::queue::UploadQueue>> {
        self.upload_queue.as_ref()
    }
    /// Whether hub tool calls pass the approval gate; see [`crate::permission::approval_gate_for`].
    pub fn tool_approval(&self) -> crate::permission::ToolApprovalGate {
        self.tool_approval
    }
    /// Return the per-session `events.jsonl` writer for `session_id`, opened
    /// and cached on first use under `workspace_home/sessions/{session_id}/`.
    pub(crate) fn session_event_writer(
        &self,
        session_id: &str,
    ) -> xai_grok_session_events::EventWriter {
        get_or_open_session_writer(
            self.events_enabled,
            &self.session_event_writers,
            &self.workspace_home,
            session_id,
        )
    }
    /// Like `session_event_writer` but never opens a new writer.
    /// Returns `None` if the session was never opened or already evicted.
    #[allow(dead_code)]
    pub(crate) fn session_event_writer_cached(
        &self,
        session_id: &str,
    ) -> Option<xai_grok_session_events::EventWriter> {
        if !self.events_enabled {
            return None;
        }
        self.session_event_writers
            .get(session_id)
            .map(|w| w.value().clone())
    }
    /// Resolved owner identity of this workspace.
    pub(crate) fn identity(&self) -> &crate::upload::environment::WorkspaceIdentity {
        &self.identity
    }
    /// Stable hub server id (`--server-id`), if a hub config is present.
    pub(crate) fn server_id(&self) -> Option<String> {
        self.hub_config.as_ref().and_then(|c| c.server_id.clone())
    }
    /// Auth provider used for xAI service calls.
    pub fn auth_provider(&self) -> Option<&xai_computer_hub_sdk::SharedAuthProvider> {
        self.auth_provider.as_ref()
    }
    /// Parse the opaque [`server_metadata`](Self::server_metadata) blob into
    /// the typed subset the workspace needs (`sandbox_id`).
    pub(crate) fn server_metadata_typed(&self) -> crate::config::WorkspaceServerMetadata {
        self.server_metadata
            .as_ref()
            .map(crate::config::WorkspaceServerMetadata::from_metadata)
            .unwrap_or_default()
    }
    pub fn default_tool_config(&self) -> &ToolServerConfig {
        &self.default_tool_config
    }
    pub fn respect_gitignore(&self) -> bool {
        self.respect_gitignore
    }
    pub fn memory_config(&self) -> Option<&MemoryConfig> {
        self.memory_config.as_ref()
    }
    pub fn mcp_tools_snapshot(&self) -> Arc<Vec<ToolConfig>> {
        self.mcp_tools_snapshot.load_full()
    }
    /// The tool server, if a server connection is active. Uses `try_lock` to avoid blocking on the async mutex from synchronous contexts.
    pub fn hub_server(&self) -> Option<xai_computer_hub_sdk::ToolServer> {
        self.hub_handle
            .try_lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|h| h.server.clone()))
    }
    /// Like [`Self::hub_server`] but awaits the `hub_handle` lock instead of
    /// returning `None` on contention.
    pub async fn hub_server_blocking(&self) -> Option<xai_computer_hub_sdk::ToolServer> {
        self.hub_handle
            .lock()
            .await
            .as_ref()
            .map(|h| h.server.clone())
    }
    /// Current snapshot of hub-provided tool configs (consumer direction).
    pub fn hub_tools_snapshot(&self) -> Arc<Vec<ToolConfig>> {
        self.hub_tools_snapshot.load_full()
    }
    /// Compose a session's tool `ctx.notification_handle` as a fan-out of the connection-level activity feed and the opt-in `system.notify` sender.
    /// Only the `system.notify` leg reaches a client, so the fan-out cannot wake the client twice.
    pub(crate) fn compose_session_notification_handle(
        &self,
        system_notify_handle: Option<ToolNotificationHandle>,
    ) -> Option<ToolNotificationHandle> {
        let activity = self.activity_notify_handle.load_full().as_ref().clone();
        match (activity, system_notify_handle) {
            (None, None) => None,
            (Some(a), None) => Some(a),
            (None, Some(s)) => Some(s),
            (Some(a), Some(s)) => Some(ToolNotificationHandle::tee(vec![a, s])),
        }
    }
    pub fn activity_tracker(&self) -> &std::sync::Arc<crate::activity::ActivityTracker> {
        &self.activity_tracker
    }
    pub fn fuzzy_searches(
        &self,
    ) -> &std::sync::Arc<tokio::sync::Mutex<crate::file_system::FuzzySearchManager>> {
        &self.fuzzy_searches
    }
    pub fn subscribe_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<xai_grok_workspace_types::WorkspaceEvent> {
        self.events.subscribe()
    }
    pub fn codebase_indexes(
        &self,
    ) -> &std::sync::Arc<parking_lot::Mutex<crate::file_system::CodebaseIndexManager>> {
        &self.codebase_indexes
    }
    /// Skill discovery configuration (extra paths and ignore prefixes).
    pub fn skills_config(&self) -> &crate::discovery::SkillsConfig {
        &self.skills_config
    }
    /// Plugin discovery configuration (CLI dirs, config paths,
    /// disabled/enabled lists).
    pub fn plugin_discovery_config(&self) -> &crate::discovery::PluginDiscoveryConfig {
        &self.plugin_discovery_config
    }
    /// Re-resolve every session's toolset and emit `ToolsChanged` events. Appropriate for spawned async tasks where notifications must not be silently lost.
    pub(crate) async fn re_resolve_all_sessions(
        self: &Arc<Self>,
        source: &str,
        use_async_lock: bool,
    ) -> usize {
        use crate::session::swap_policy::{
            SessionSnapshot, SwapAction, SwapDecision, SwapPolicy, SwapTrigger,
            record_swap_decision,
        };
        let trigger = SwapTrigger::from_rebuild_source(source);
        let mcp_snap = self.mcp_tools_snapshot.load_full();
        let hub_snap = self.hub_tools_snapshot.load_full();
        let sessions: Vec<(String, Arc<WorkspaceSession>)> = {
            let guard = self.sessions.read();
            guard
                .iter()
                .map(|(id, s)| (id.clone(), s.clone()))
                .collect()
        };
        let mut rebuilt = 0usize;
        for (sid, session) in sessions {
            let guard = if use_async_lock {
                session.update_lock.lock().await
            } else {
                match session.update_lock.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        tracing::trace!(
                            session = %sid,
                            source = %source,
                            "skipping rebuild: session update_lock held"
                        );
                        continue;
                    }
                }
            };
            let snapshot =
                SessionSnapshot::capture_for_rebuild(&session, &self.activity_tracker).await;
            match SwapPolicy::evaluate(&snapshot, trigger) {
                SwapDecision::Apply => {}
                SwapDecision::Skip(reason) => {
                    record_swap_decision(
                        &self.activity_tracker,
                        trigger,
                        &sid,
                        SwapAction::Skipped(reason),
                    );
                    tracing::warn!(
                        session = %sid,
                        source = %source,
                        "skipping rebuild: toolset terminal backend is externally \
                         owned (local bind)"
                    );
                    drop(guard);
                    continue;
                }
                decision @ (SwapDecision::Reuse | SwapDecision::Defer(_)) => {
                    debug_assert!(
                        false,
                        "snapshot rebuild produced a non-rebuild decision: {decision:?}"
                    );
                    tracing::error!(
                        session = %sid,
                        source = %source,
                        ?decision,
                        "skipping rebuild: snapshot rebuild policy returned a \
                         non-rebuild decision (policy regression)"
                    );
                    drop(guard);
                    continue;
                }
            }
            let baseline = (*session.effective_tool_config()).clone();
            match crate::session::tool_config::resolve_session_toolset_rebuild(
                baseline,
                session.capability_mode(),
                &mcp_snap,
                &hub_snap,
                session.cwd().to_path_buf(),
                session.session_env().clone(),
                &sid,
                self.session_factory.as_ref(),
                Some(self.local_registry.clone()),
                self.lsp.clone(),
                session.viewer_ctx().cloned(),
                self.compose_session_notification_handle(session.system_notify_handle()),
                session.terminal_backend().clone(),
                crate::session::tool_config::truncation_config_for_host(self.host_kind),
            ) {
                Ok((effective, toolset)) => {
                    session
                        .replace_carrying_browser_service(Arc::new(effective), toolset)
                        .await;
                    session.clear_stale_resolve();
                    record_swap_decision(
                        &self.activity_tracker,
                        trigger,
                        &sid,
                        SwapAction::Applied,
                    );
                    let _ =
                        self.events
                            .send(xai_grok_workspace_types::WorkspaceEvent::ToolsChanged {
                                session_id: sid,
                            });
                    rebuilt += 1;
                }
                Err(e) => {
                    session.mark_stale_resolve();
                    record_swap_decision(
                        &self.activity_tracker,
                        trigger,
                        &sid,
                        SwapAction::ApplyFailed,
                    );
                    tracing::warn!(
                        session = %sid,
                        source = %source,
                        error = %e,
                        "snapshot rebuild failed for session"
                    );
                }
            }
            drop(guard);
        }
        rebuilt
    }
}
/// Core get-or-open logic for a session's `events.jsonl` writer, factored out of [`WorkspaceShared::session_event_writer`].
/// The split lets the `enabled` gate be unit-tested without touching process environment. - `enabled == false`: returns [`EventWriter::noop()`]; the `writers` map and the filesystem are left untouched (legacy behaviour preserved).
pub(crate) fn get_or_open_session_writer(
    enabled: bool,
    writers: &dashmap::DashMap<String, xai_grok_session_events::EventWriter>,
    workspace_home: &Path,
    session_id: &str,
) -> xai_grok_session_events::EventWriter {
    use xai_grok_session_events::EventWriter;
    if !enabled {
        return EventWriter::noop();
    }
    if let Some(existing) = writers.get(session_id) {
        return existing.value().clone();
    }
    let sessions_root = workspace_home.join("sessions");
    let dir = sessions_root.join(session_id);
    let create = xai_grok_config::create_dir_all_owner_only(&dir);
    xai_grok_config::set_dir_owner_only(&sessions_root);
    if let Err(e) = create {
        tracing::warn!(
            session_id = %session_id,
            dir = %dir.display(),
            error = %e,
            "failed to create session event dir; events.jsonl disabled for this session (will retry on next use)"
        );
        return EventWriter::noop();
    }
    let writer = EventWriter::open(&dir);
    writers
        .entry(session_id.to_owned())
        .or_insert(writer)
        .clone()
}
#[cfg(test)]
mod tests {
    use super::get_or_open_session_writer;
    use dashmap::DashMap;
    use xai_grok_session_events::{Event, EventWriter};
    fn count_lines(path: &std::path::Path) -> usize {
        std::fs::read_to_string(path)
            .unwrap()
            .trim()
            .lines()
            .count()
    }
    #[test]
    fn flag_off_returns_noop_and_creates_nothing() {
        let home = tempfile::tempdir().unwrap();
        let writers: DashMap<String, EventWriter> = DashMap::new();
        let w = get_or_open_session_writer(false, &writers, home.path(), "sess-a");
        w.emit(Event::ToolStarted {
            tool_name: "read_file".into(),
        });
        assert!(writers.is_empty(), "flag-off must not cache a writer");
        let sess_dir = home.path().join("sessions").join("sess-a");
        assert!(
            !sess_dir.exists(),
            "flag-off must not create the session dir or events.jsonl"
        );
    }
    /// Session-derived content outside ~/.grok/sessions: same owner-only rule,
    /// including healing a loose pre-existing root from older builds.
    #[cfg(unix)]
    #[test]
    fn flag_on_creates_owner_only_session_event_dir() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let writers: DashMap<String, EventWriter> = DashMap::new();
        let root = home.path().join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = get_or_open_session_writer(true, &writers, home.path(), "sess-perm");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode(&root.join("sess-perm")),
            0o700,
            "session event dir must be 0700"
        );
        assert_eq!(mode(&root), 0o700, "loose sessions root must heal to 0700");
    }
    #[test]
    fn flag_on_opens_and_writes_real_content() {
        let home = tempfile::tempdir().unwrap();
        let writers: DashMap<String, EventWriter> = DashMap::new();
        let w = get_or_open_session_writer(true, &writers, home.path(), "sess-b");
        w.emit(Event::YoloToggled { enabled: true });
        assert_eq!(writers.len(), 1, "flag-on must cache the opened writer");
        let path = home
            .path()
            .join("sessions")
            .join("sess-b")
            .join("events.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v.get("type").and_then(|v| v.as_str()), Some("yolo_toggled"));
        assert_eq!(v.get("enabled"), Some(&serde_json::json!(true)));
        assert!(v.get("ts").and_then(|v| v.as_str()).is_some());
    }
    #[test]
    fn second_call_reuses_one_cache_entry() {
        let home = tempfile::tempdir().unwrap();
        let writers: DashMap<String, EventWriter> = DashMap::new();
        get_or_open_session_writer(true, &writers, home.path(), "sess-c").emit(
            Event::ToolStarted {
                tool_name: "a".into(),
            },
        );
        get_or_open_session_writer(true, &writers, home.path(), "sess-c").emit(
            Event::ToolStarted {
                tool_name: "b".into(),
            },
        );
        assert_eq!(writers.len(), 1, "same session must reuse one cache entry");
        let path = home
            .path()
            .join("sessions")
            .join("sess-c")
            .join("events.jsonl");
        assert_eq!(count_lines(&path), 2);
    }
    #[test]
    fn reopen_after_restart_appends() {
        let home = tempfile::tempdir().unwrap();
        {
            let writers: DashMap<String, EventWriter> = DashMap::new();
            get_or_open_session_writer(true, &writers, home.path(), "sess-d").emit(
                Event::ToolStarted {
                    tool_name: "before-restart".into(),
                },
            );
        }
        {
            let writers: DashMap<String, EventWriter> = DashMap::new();
            get_or_open_session_writer(true, &writers, home.path(), "sess-d").emit(
                Event::ToolStarted {
                    tool_name: "after-restart".into(),
                },
            );
        }
        let path = home
            .path()
            .join("sessions")
            .join("sess-d")
            .join("events.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.trim().lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "re-open after restart must append, preserving the earlier line"
        );
        let [first_line, second_line] = lines.as_slice() else {
            panic!("expected two event lines: {lines:?}");
        };
        let first: serde_json::Value = serde_json::from_str(first_line).unwrap();
        assert_eq!(
            first.get("tool_name").and_then(|v| v.as_str()),
            Some("before-restart")
        );
        let second: serde_json::Value = serde_json::from_str(second_line).unwrap();
        assert_eq!(
            second.get("tool_name").and_then(|v| v.as_str()),
            Some("after-restart")
        );
    }
}
