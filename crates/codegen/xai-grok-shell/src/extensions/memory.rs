//! Extension handlers for `x.ai/compact_conversation`, `x.ai/memory/flush`, `x.ai/memory/rewrite`,
//! `x.ai/memory/list`, `x.ai/memory/toggle`, and `x.ai/memory/forget`.
//! `memory/rewrite` turns a raw memory note into structured markdown with a one-shot LLM call.
//! `memory/list` and `memory/toggle` back the `/memory` modal without running a prompt turn, so
//! opening it or flipping memory writes nothing to scrollback.
//! `memory/forget` deletes one note from the `/memory` modal through the store's tombstone path.
//! `memory/flush` and `memory/dream` back the pager-local `/flush` and `/dream` commands; each
//! returns a typed disposition so the client can render the outcome.

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::notification::{MemoryDisabledReason, MemoryFileInfo};
use super::{ExtResult, parse_params, to_raw_response};
use crate::agent::MvpAgent;
use crate::session::{
    CompactConversationRequest, CompactConversationResponse, SessionCommand, SessionHandle,
};
pub use xai_grok_shell_core::extensions::memory::*;

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        m if m.starts_with("x.ai/compact_conversation") => handle_compact(agent, args).await,
        MEMORY_FLUSH_METHOD => handle_flush(agent, args).await,
        MEMORY_DREAM_METHOD => handle_dream(agent, args).await,
        MEMORY_REWRITE_METHOD => handle_rewrite(agent, args).await,
        MEMORY_LIST_METHOD => handle_list(agent, args).await,
        MEMORY_TOGGLE_METHOD => handle_toggle(agent, args).await,
        MEMORY_FORGET_METHOD => handle_forget(agent, args).await,
        _ => Err(acp::Error::method_not_found()),
    }
}

fn resident_session(agent: &MvpAgent, session_id: String) -> Result<SessionHandle, acp::Error> {
    agent
        .resident_handle(&acp::SessionId::new(session_id.clone()))
        .ok_or_else(|| {
            acp::Error::invalid_params().data(format!("session not found: {session_id}"))
        })
}

async fn handle_list(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryListRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session
        .cmd_tx
        .send(SessionCommand::MemoryList { respond_to: tx });
    let listing = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(e))?;
    to_raw_response(&listing)
}

async fn handle_toggle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryToggleRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::MemoryToggle {
        enabled: req.enabled,
        respond_to: tx,
    });
    let response = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&response)
}

async fn handle_forget(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryForgetRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::MemoryForget {
        path: req.path,
        expected_content_hash: req.expected_content_hash,
        respond_to: tx,
    });
    let response = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&response)
}

async fn handle_compact(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: CompactConversationRequest = parse_params(args)?;
    // send over the compact query here properly
    let sid: acp::SessionId = req.session_id.into();
    let session_handle = agent.resident_handle(&sid);
    let (tx, rx) = oneshot::channel();
    if let Some(session) = session_handle {
        let _ = session.cmd_tx.send(SessionCommand::CompactSession {
            user_context: req.user_context,
            respond_to: tx,
        });
    }
    // Pass the session error through; rewrapping buries the detail in a Debug dump.
    rx.await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))??;
    to_raw_response(&CompactConversationResponse {})
}

async fn handle_flush(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryFlushRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session
        .cmd_tx
        .send(SessionCommand::FlushMemory { respond_to: tx });
    let response = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&response)
}

async fn handle_dream(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryFlushRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session
        .cmd_tx
        .send(SessionCommand::MemoryDream { respond_to: tx });
    let response = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&response)
}

async fn handle_rewrite(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryRewriteRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::RewriteMemoryNote {
        raw_text: req.raw_text,
        context_summary: req.context_summary,
        respond_to: tx,
    });
    let rewritten = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(e))?;
    to_raw_response(&MemoryRewriteResponse { rewritten })
}
