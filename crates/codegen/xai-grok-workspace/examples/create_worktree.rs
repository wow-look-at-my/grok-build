//! Drive the shipped worktree creation path against a real repository.
//!
//!     cargo run -p xai-grok-workspace --example create_worktree -- create <source> <label>
//!     cargo run -p xai-grok-workspace --example create_worktree -- remove <path>
//!
//! `create` runs the two RPC entry points `grok --worktree=<label>` runs, in the
//! same order: `prepare_worktree_creation` picks the destination and
//! `create_worktree_streaming` builds the checkout and registers it. It prints
//! the destination, which is what the pager shows and cd's into.
//!
//! `remove` runs `remove_worktree`, the other half of the same path, so a
//! throwaway checkout can be discarded the way the product discards one.

use std::path::Path;

use xai_grok_workspace::worktree::{
    BackgroundCopyContext, CreateWorktreeRequest, RemoveWorktreeRequest,
    WorktreeNotificationSender, WorktreeStatus, create_worktree_streaming,
    prepare_worktree_creation, remove_worktree,
};

#[derive(Clone)]
struct PrintNotifications;

#[async_trait::async_trait]
impl WorktreeNotificationSender for PrintNotifications {
    async fn send_worktree_status(&self, progress: WorktreeStatus) {
        println!("notify: {progress:?}");
    }
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  create_worktree create <source-path> <label>\n  \
         create_worktree remove <worktree-path>"
    );
    std::process::exit(2);
}

fn env_session_id() -> String {
    format!("example-{}", std::process::id())
}

async fn create(source_path: String, label: String) -> anyhow::Result<()> {
    let req = CreateWorktreeRequest {
        session_id: env_session_id(),
        source_path,
        worktree_path: None,
        copy_mode: Default::default(),
        git_ref: None,
        copy_ignored_in_background: false,
        ignored_skip_patterns: Vec::new(),
        worktree_type: None,
        label: Some(label),
<<<<<<< HEAD
        grove_worktree: None,
        grove_gate_source: None,
        resolved_source_git_root: None,
=======
>>>>>>> origin/master
    };

    let prepared = prepare_worktree_creation(&req).await;
    println!("prepare: spawn_task={}", prepared.spawn_task);
    match &prepared.response {
        Ok(response) => println!("prepare destination: {response:?}"),
        Err(e) => anyhow::bail!("prepare_worktree_creation failed: {e:#}"),
    }

    let status = create_worktree_streaming(&req, &PrintNotifications).await;
    println!("created: {status:?}");
    Ok(())
}

async fn remove(worktree_path: String) -> anyhow::Result<()> {
    let req = RemoveWorktreeRequest {
        worktree_path: Some(worktree_path),
        id_or_path: None,
        force: true,
        dry_run: false,
    };
    let response = remove_worktree(&req, &BackgroundCopyContext::default()).await?;
    println!("removed: {response:?}");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (verb, arg1, arg2) = match (args.next(), args.next(), args.next()) {
        (Some(verb), Some(a), b) => (verb, a, b),
        _ => usage(),
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    // `create_worktree_streaming` uses `spawn_local` for its background copy, so
    // this needs a LocalSet rather than a plain block_on.
    let local = tokio::task::LocalSet::new();

    match verb.as_str() {
        "create" => {
            let label = arg2.unwrap_or_else(|| usage());
            // Sanity: the source must be a path, not a typo'd label.
            if !Path::new(&arg1).is_dir() {
                anyhow::bail!("source path is not a directory: {arg1}");
            }
            local.block_on(&runtime, create(arg1, label))
        }
        "remove" => {
            if arg2.is_some() {
                usage();
            }
            local.block_on(&runtime, remove(arg1))
        }
        other => {
            eprintln!("unknown verb: {other}");
            usage()
        }
    }
}
