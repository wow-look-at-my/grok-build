//! Bridges xai-grok-tools LspBackend trait to LspManager.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex as TokioMutex;

use async_lsp::LanguageServer;
use async_lsp::lsp_types::{
    self, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, HoverParams, Location, ReferenceContext, ReferenceParams,
    SymbolInformation, TextDocumentIdentifier, TextDocumentPositionParams, WorkspaceSymbolParams,
};

use super::{LspToolInput, LspToolResult};

use super::config::REQUEST_TIMEOUT;
use super::format::{
    flatten_document_symbols, format_locations_labeled, format_symbols, markup_string_to_text,
};
use super::manager::LspManager;
use super::{LspError, file_uri, text_document_position};

// ── Public adapter ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum StartupState {
    NotStarted,
    Starting,
    Ready,
    Failed(String),
}

struct StartupCoordinator {
    state: TokioMutex<StartupState>,
    notify: tokio::sync::Notify,
    pre_ready_file_changes: TokioMutex<HashMap<PathBuf, (Option<String>, super::DiskChangeKind)>>,
}

pub struct LspBackendAdapter {
    lsp_manager: Arc<tokio::sync::Mutex<LspManager>>,
    startup: Arc<StartupCoordinator>,
}

impl LspBackendAdapter {
    pub fn new(lsp_manager: Arc<tokio::sync::Mutex<LspManager>>) -> Self {
        Self {
            lsp_manager,
            startup: Arc::new(StartupCoordinator {
                state: TokioMutex::new(StartupState::NotStarted),
                notify: tokio::sync::Notify::new(),
                pre_ready_file_changes: TokioMutex::new(HashMap::new()),
            }),
        }
    }

    fn spawn_bootstrap_task(
        lsp_manager: Arc<tokio::sync::Mutex<LspManager>>,
        startup: Arc<StartupCoordinator>,
    ) {
        #[allow(clippy::disallowed_methods)]
        tokio::spawn(async move {
            // Guarded, because the state below is what a waiter is waiting on:
            // `ensure_ready` parks on `notify` for as long as the state reads
            // `Starting`, so a bootstrap that dies mid-flight has to move the
            // state anyway and say why.
            let result = match crate::util::detached::guarded(
                "lsp bootstrap",
                bootstrap_lsp(lsp_manager, startup.clone()),
            )
            .await
            {
                Ok(result) => result,
                Err(panic) => Err(format!("the LSP bootstrap task panicked: {panic}")),
            };
            let mut state = startup.state.lock().await;
            *state = match result {
                Ok(()) => StartupState::Ready,
                Err(error) => StartupState::Failed(error),
            };
            drop(state);
            startup.notify.notify_waiters();
        });
    }

    async fn ensure_started_inner(&self) {
        Self::ensure_started_with_state(self.lsp_manager.clone(), self.startup.clone()).await;
    }

    async fn ensure_started_with_state(
        lsp_manager: Arc<tokio::sync::Mutex<LspManager>>,
        startup: Arc<StartupCoordinator>,
    ) {
        let mut state = startup.state.lock().await;
        if matches!(&*state, StartupState::NotStarted) {
            *state = StartupState::Starting;
            drop(state);
            LspBackendAdapter::spawn_bootstrap_task(lsp_manager, startup);
        }
    }
}

async fn bootstrap_lsp(
    lsp_manager: Arc<tokio::sync::Mutex<LspManager>>,
    startup: Arc<StartupCoordinator>,
) -> Result<(), String> {
    let pending_changes: Vec<(PathBuf, Option<String>, super::DiskChangeKind)> = {
        let mut pending = startup.pre_ready_file_changes.lock().await;
        pending
            .drain()
            .map(|(path, (content, kind))| (path, content, kind))
            .collect()
    };

    let restartable = {
        let mut mgr = lsp_manager.lock().await;
        mgr.ensure_initialized().await;
        if mgr.clients.is_empty() {
            return Err("No LSP servers started successfully.".to_string());
        }
        for (path, content, kind) in &pending_changes {
            mgr.notify_file_event(path, content.as_deref(), *kind);
        }
        mgr.restartable_servers()
    };
    for name in restartable {
        // Hand the monitor a `Weak` so it never keeps the manager (and its language-server children).
        let mgr_weak = Arc::downgrade(&lsp_manager);
        // Nothing polls this monitor: a server that dies with no monitor left
        // stops being diagnosed.
        #[allow(clippy::disallowed_methods)]
        tokio::spawn(crate::util::detached::fire_and_forget(
            "lsp restart monitor",
            crate::implementations::lsp::restart_monitor(mgr_weak, name),
        ));
    }
    Ok(())
}

impl Drop for LspBackendAdapter {
    fn drop(&mut self) {
        tracing::debug!("LspBackendAdapter dropping, initiating LSP shutdown");
        let mgr = self.lsp_manager.clone();
        let _ = tokio::runtime::Handle::try_current().map(|handle| {
            handle.spawn(async move {
                mgr.lock().await.shutdown().await;
            })
        });
    }
}

#[async_trait::async_trait]
impl super::LspBackend for LspBackendAdapter {
    fn ensure_started_background(&self) {
        let lsp_manager = self.lsp_manager.clone();
        let startup = self.startup.clone();
        // A warm-up needs a runtime to run on.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::debug!("no tokio runtime: skipping the LSP startup warm-up");
            return;
        };
        handle.spawn(async move {
            // Guarded, because this is the round that leaves the state
            // `Starting` for the bootstrap to replace.
            let started = crate::util::detached::guarded(
                "lsp start",
                LspBackendAdapter::ensure_started_with_state(lsp_manager, startup.clone()),
            )
            .await;
            if let Err(panic) = started {
                let mut state = startup.state.lock().await;
                *state = StartupState::Failed(format!("the LSP start task panicked: {panic}"));
                drop(state);
                startup.notify.notify_waiters();
            }
        });
    }

    async fn ensure_ready(&self) -> Result<(), String> {
        self.ensure_started_inner().await;
        loop {
            let notified = {
                let state = self.startup.state.lock().await;
                match &*state {
                    StartupState::Ready => return Ok(()),
                    StartupState::Failed(error) => return Err(error.clone()),
                    StartupState::NotStarted => continue,
                    StartupState::Starting => self.startup.notify.notified(),
                }
            };
            notified.await;
        }
    }

    fn is_ready(&self) -> bool {
        self.startup
            .state
            .try_lock()
            .map(|state| matches!(&*state, StartupState::Ready))
            .unwrap_or(false)
    }

    async fn dispatch(&self, input: &LspToolInput) -> LspToolResult {
        use super::LspOperation;

        if let Err(error) = self.ensure_ready().await {
            return err_result(format!("LSP startup failed: {error}"));
        }

        // Brief lock: auto-open + clone socket(s). Then drop lock.
        let sockets = {
            let mut mgr = self.lsp_manager.lock().await;
            match input.operation {
                LspOperation::WorkspaceSymbol => {
                    if mgr.clients.is_empty() {
                        return err_result("No LSP servers are running.".into());
                    }
                    DispatchSockets::All(mgr.all_sockets())
                }
                _ => {
                    let path = match input.file_path.as_deref() {
                        Some(fp) => PathBuf::from(fp),
                        None => return err_result("Required: file_path.".into()),
                    };
                    match mgr.socket_for_file(&path).await {
                        Some(s) => DispatchSockets::One(s),
                        None => {
                            return err_result(format!(
                                "No LSP server configured for {}",
                                path.display()
                            ));
                        }
                    }
                }
            }
        };
        // Lock dropped — dispatch on cloned socket(s).
        dispatch_on_sockets(input, sockets).await
    }

    async fn drain_diagnostics(
        &self,
        timeout: std::time::Duration,
    ) -> Option<super::manager::DiagnosticsSummary> {
        if !self.is_ready() {
            return None;
        }
        super::manager::drain_lsp_diagnostics(&self.lsp_manager, timeout).await
    }

    async fn notify_file_changed(&self, path: &std::path::Path, content: &str) {
        self.notify_file_event(path, Some(content), super::DiskChangeKind::Changed)
            .await;
    }

    async fn notify_file_event(
        &self,
        path: &std::path::Path,
        content: Option<&str>,
        kind: super::DiskChangeKind,
    ) {
        if self.is_ready() {
            self.lsp_manager
                .lock()
                .await
                .notify_file_event(path, content, kind);
        } else {
            self.startup
                .pre_ready_file_changes
                .lock()
                .await
                .insert(path.to_path_buf(), (content.map(str::to_string), kind));
        }
    }

    async fn read_diagnostics(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Vec<super::FileDiagnosticEntry> {
        if self.ensure_ready().await.is_err() {
            return vec![];
        }

        // Open files that aren't tracked yet so the LSP can analyze them.
        {
            let mut mgr = self.lsp_manager.lock().await;
            for path in paths {
                let _ = mgr.socket_for_file(path).await;
            }
        }

        // Wait briefly after opening files.
        let notify = {
            let mgr = self.lsp_manager.lock().await;
            mgr.diagnostics_ready.clone()
        };
        let notified = notify.notified();
        let _ = tokio::time::timeout(std::time::Duration::from_millis(1000), notified).await;

        // Collect diagnostics from all clients for the requested paths.
        let mgr = self.lsp_manager.lock().await;
        let mut results = Vec::new();
        for path in paths {
            let uri = match super::file_uri(path) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let uri_str = uri.to_string();
            let mut file_diagnostics = Vec::new();

            for client in mgr.clients.values() {
                for d in client.diagnostics.items(&uri_str) {
                    let severity = match d.severity {
                        Some(async_lsp::lsp_types::DiagnosticSeverity::ERROR) => {
                            super::DiagnosticSeverityLevel::Error
                        }
                        Some(async_lsp::lsp_types::DiagnosticSeverity::WARNING) => {
                            super::DiagnosticSeverityLevel::Warning
                        }
                        _ => continue,
                    };
                    file_diagnostics.push(super::DiagnosticEntry {
                        severity,
                        // LSP uses 0-based positions; convert to 1-based for display (L{line}:{column}).
                        line: d.range.start.line + 1,
                        column: d.range.start.character + 1,
                        message: d.message.clone(),
                        source: d.source.clone(),
                        code: None,
                        is_stale: false,
                    });
                }
            }

            results.push(super::FileDiagnosticEntry {
                path: path.display().to_string(),
                diagnostics: file_diagnostics,
            });
        }
        results
    }
}

// ── Internal types ──────────────────────────────────────────────────────

enum DispatchSockets {
    One(async_lsp::ServerSocket),
    All(Vec<async_lsp::ServerSocket>),
}

fn err_result(msg: String) -> LspToolResult {
    LspToolResult {
        text: msg,
        is_error: true,
    }
}

fn ok_result(text: String) -> LspToolResult {
    LspToolResult {
        text,
        is_error: false,
    }
}

fn require_position(input: &LspToolInput) -> Result<(PathBuf, TextDocumentPositionParams), String> {
    let (Some(fp), Some(line), Some(character)) = (&input.file_path, input.line, input.character)
    else {
        return Err("Required: file_path, line, character.".into());
    };
    let path = PathBuf::from(fp);
    let params = text_document_position(&path, line, character).map_err(|e| format!("{e}"))?;
    Ok((path, params))
}

/// Awaits an LSP request future with a standard timeout and error mapping.
async fn timed_request<F, T, E>(fut: F) -> Result<T, LspError>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    tokio::time::timeout(REQUEST_TIMEOUT, fut)
        .await
        .map_err(|_| LspError::RequestFailed("request timed out".into()))?
        .map_err(|e| LspError::RequestFailed(format!("{e}")))
}

/// Distinguishes validation errors (missing params) from LSP protocol errors.
enum DispatchError {
    /// Missing or invalid input parameters.
    Validation(String),
    /// LSP request failed or timed out.
    Lsp(LspError),
}

impl From<String> for DispatchError {
    fn from(msg: String) -> Self {
        Self::Validation(msg)
    }
}

impl From<LspError> for DispatchError {
    fn from(e: LspError) -> Self {
        Self::Lsp(e)
    }
}

// ── Router ──────────────────────────────────────────────────────────────

async fn dispatch_on_sockets(input: &LspToolInput, sockets: DispatchSockets) -> LspToolResult {
    use super::LspOperation;

    match sockets {
        DispatchSockets::One(mut socket) => {
            let result = match input.operation {
                LspOperation::GoToDefinition => dispatch_goto(input, &mut socket, true).await,
                LspOperation::GoToImplementation => dispatch_goto(input, &mut socket, false).await,
                LspOperation::FindReferences => dispatch_references(input, &mut socket).await,
                LspOperation::Hover => dispatch_hover(input, &mut socket).await,
                LspOperation::DocumentSymbol => dispatch_document_symbols(input, &mut socket).await,
                LspOperation::WorkspaceSymbol => unreachable!(),
            };
            match result {
                Ok(text) => ok_result(text),
                Err(DispatchError::Validation(msg)) => err_result(msg),
                Err(DispatchError::Lsp(e)) => {
                    tracing::warn!(error = %e, "LSP tool failed");
                    err_result(format!("LSP error: {e}"))
                }
            }
        }
        DispatchSockets::All(sockets) => dispatch_workspace_symbols(input, sockets).await,
    }
}

// ── Per-operation helpers ───────────────────────────────────────────────

/// Handles both GoToDefinition and GoToImplementation (same params/response shape).
async fn dispatch_goto(
    input: &LspToolInput,
    socket: &mut async_lsp::ServerSocket,
    is_definition: bool,
) -> Result<String, DispatchError> {
    let (_path, pos) = require_position(input)?;
    let params = GotoDefinitionParams {
        text_document_position_params: pos,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let label = if is_definition {
        "Definition"
    } else {
        "Implementations"
    };
    let fut = if is_definition {
        socket.definition(params)
    } else {
        socket.implementation(params)
    };
    let response = timed_request(fut).await?;
    let locs = parse_goto_response(response);
    Ok(format_locations_labeled(label, &locs))
}

async fn dispatch_references(
    input: &LspToolInput,
    socket: &mut async_lsp::ServerSocket,
) -> Result<String, DispatchError> {
    let (_path, pos) = require_position(input)?;
    let params = ReferenceParams {
        text_document_position: pos,
        context: ReferenceContext {
            include_declaration: true,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let result = timed_request(socket.references(params)).await?;
    Ok(format_locations_labeled(
        "References",
        &result.unwrap_or_default(),
    ))
}

async fn dispatch_hover(
    input: &LspToolInput,
    socket: &mut async_lsp::ServerSocket,
) -> Result<String, DispatchError> {
    let (_path, pos) = require_position(input)?;
    let params = HoverParams {
        text_document_position_params: pos,
        work_done_progress_params: Default::default(),
    };
    let result = timed_request(socket.hover(params)).await?;
    Ok(result
        .map(|h| match h.contents {
            lsp_types::HoverContents::Scalar(ms) => markup_string_to_text(ms),
            lsp_types::HoverContents::Array(arr) => arr
                .into_iter()
                .map(markup_string_to_text)
                .collect::<Vec<_>>()
                .join("\n"),
            lsp_types::HoverContents::Markup(mc) => mc.value,
        })
        .unwrap_or_else(|| "No hover information available.".to_string()))
}

async fn dispatch_document_symbols(
    input: &LspToolInput,
    socket: &mut async_lsp::ServerSocket,
) -> Result<String, DispatchError> {
    let Some(ref fp) = input.file_path else {
        return Err(DispatchError::Validation("Required: file_path.".into()));
    };
    let uri = file_uri(Path::new(fp))?;
    let params = DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let result = timed_request(socket.document_symbol(params)).await?;
    Ok(match result {
        Some(DocumentSymbolResponse::Flat(s)) => format_symbols(&s),
        Some(DocumentSymbolResponse::Nested(n)) => {
            let mut flat = Vec::new();
            flatten_document_symbols(&n, &uri, &mut flat);
            format_symbols(&flat)
        }
        None => format_symbols(&[]),
    })
}

async fn dispatch_workspace_symbols(
    input: &LspToolInput,
    sockets: Vec<async_lsp::ServerSocket>,
) -> LspToolResult {
    let Some(ref query) = input.query else {
        return err_result("Required: query (string).".into());
    };
    let mut all_symbols = Vec::new();
    let mut last_err: Option<String> = None;
    for mut s in sockets {
        let params = WorkspaceSymbolParams {
            query: query.to_string(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, s.symbol(params)).await {
            Ok(Ok(Some(lsp_types::WorkspaceSymbolResponse::Flat(symbols)))) => {
                all_symbols.extend(symbols)
            }
            Ok(Ok(Some(lsp_types::WorkspaceSymbolResponse::Nested(ws)))) => {
                all_symbols.extend(ws.into_iter().map(workspace_symbol_to_info));
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => last_err = Some(format!("{e}")),
            Err(_) => last_err = Some("request timed out".into()),
        }
    }
    if all_symbols.is_empty() {
        match last_err {
            Some(msg) => err_result(format!("LSP error: {msg}")),
            None => ok_result(format_symbols(&[])),
        }
    } else {
        ok_result(format_symbols(&all_symbols))
    }
}

// ── Response converters ─────────────────────────────────────────────────

fn parse_goto_response(response: Option<GotoDefinitionResponse>) -> Vec<Location> {
    match response {
        Some(GotoDefinitionResponse::Scalar(l)) => vec![l],
        Some(GotoDefinitionResponse::Array(l)) => l,
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
        None => vec![],
    }
}

fn workspace_symbol_to_info(ws: lsp_types::WorkspaceSymbol) -> SymbolInformation {
    let loc = match ws.location {
        lsp_types::OneOf::Left(l) => l,
        lsp_types::OneOf::Right(d) => Location {
            uri: d.uri,
            range: Default::default(),
        },
    };
    #[allow(deprecated)]
    SymbolInformation {
        name: ws.name,
        kind: ws.kind,
        tags: ws.tags,
        deprecated: None,
        location: loc,
        container_name: ws.container_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implementations::lsp::LspBackend;
    use std::time::Duration;

    fn adapter(
        servers: std::collections::BTreeMap<String, super::super::config::LspServerConfig>,
    ) -> LspBackendAdapter {
        let manager = LspManager::new(
            servers,
            std::env::temp_dir(),
            true,
            crate::notification::ToolNotificationHandle::noop(),
        );
        LspBackendAdapter::new(Arc::new(TokioMutex::new(manager)))
    }

    /// A configured server keeps the adapter out of the "nothing to do" shape
    /// without any process being spawned: the fix under test decides whether the
    /// bootstrap task is created at all.
    fn adapter_with_one_configured_server() -> LspBackendAdapter {
        adapter(std::collections::BTreeMap::from([(
            "test-server".to_owned(),
            super::super::config::LspServerConfig::default(),
        )]))
    }

    /// A bootstrap that ends in failure still answers whoever is waiting.
    ///
    /// `ensure_ready` parks on the coordinator's `notify` for as long as the
    /// state reads `Starting`, so a bootstrap that stopped without moving the
    /// state leaves every later LSP tool call waiting on a task that already
    /// ended. A manager with no servers configured is that ending, with nothing
    /// injected.
    #[tokio::test]
    async fn a_failed_bootstrap_answers_the_waiter_instead_of_stranding_it() {
        let manager = Arc::new(TokioMutex::new(LspManager::default()));
        let adapter = LspBackendAdapter::new(manager);

        let outcome = tokio::time::timeout(Duration::from_secs(30), adapter.ensure_ready())
            .await
            .expect("the waiter must be answered, not left parked on a bootstrap that ended");
        let error = outcome.expect_err("a manager with no language servers is not ready");
        assert!(
            error.contains("No LSP servers"),
            "the failure must reach the caller as its own reason, got {error}"
        );
        assert!(!adapter.is_ready());
    }

    /// `ensure_started_background` is a warm-up, so a caller with no runtime to
    /// run it on loses the warm-up and nothing else. Spawning unconditionally
    /// made every synchronous `WorkspaceHandle` construction panic the moment
    /// the caller had any LSP server in `~/.grok/lsp.json`, which is a plain
    /// test with no Tokio runtime and a machine-dependent one besides.
    #[test]
    fn ensure_started_background_without_a_runtime_skips_the_warmup() {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "this test must have no runtime to cover the no-runtime path"
        );
        let adapter = adapter_with_one_configured_server();

        adapter.ensure_started_background();

        let state = adapter.startup.state.blocking_lock();
        assert!(
            matches!(&*state, StartupState::NotStarted),
            "the warm-up must leave the state for `ensure_ready` to drive, \
             not half-start it: {state:?}"
        );
    }

    /// Inside a runtime the warm-up still runs, so this fix does not quietly
    /// disable LSP startup on the path production takes. No server is
    /// configured, so the bootstrap reaches a verdict without spawning one.
    #[tokio::test]
    async fn ensure_started_background_inside_a_runtime_leaves_starting_state() {
        let adapter = adapter(std::collections::BTreeMap::new());

        adapter.ensure_started_background();
        for _ in 0..50 {
            let started = !matches!(
                &*adapter.startup.state.lock().await,
                StartupState::NotStarted
            );
            if started {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the warm-up never started inside a running tokio runtime");
    }
}
