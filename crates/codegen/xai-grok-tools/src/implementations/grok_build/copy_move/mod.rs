//! `copy_file` and `move_file` — first-class relocate tools.
//!
//! Relocating code is `cp` / `git mv` plus a small edit, never a full
//! rewrite of an existing file through `write` / `search_replace`.
use crate::notification::types::FileWritten;
use crate::types::output::{TextOutput, ToolOutput};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::resources::{DisplayCwd, FileSystem, NotificationHandle, resolve_model_path};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::ToolMetadata;
use std::path::{Path, PathBuf};
use std::process::Command;

const COPY_DESCRIPTION: &str = r#"Copy a file from source to destination (`cp`).

Use this to duplicate an existing file. Do not retype the file with write/search_replace.

- Parent directories of the destination are created for you.
- Fails if the destination exists unless `overwrite` is true.
- Directories are not copied; use `cp -r` via the shell for a tree."#;

const MOVE_DESCRIPTION: &str = r#"Move or rename a file (`git mv` when the source is tracked, otherwise rename).

Use this to relocate existing code. Do not retype the file with write/search_replace.

- Parent directories of the destination are created for you.
- Fails if the destination exists unless `overwrite` is true.
- Tracked git files are moved with `git mv` so history follows the path."#;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CopyMoveInput {
    #[schemars(
        description = "The path to copy or move from. Relative to the workspace or absolute."
    )]
    pub source: String,
    #[schemars(
        description = "The path to copy or move to. Relative to the workspace or absolute."
    )]
    pub destination: String,
    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_bool"
    )]
    #[schemars(description = "Replace the destination if it already exists (default false).")]
    pub overwrite: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CopyMoveOutput {
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for CopyMoveOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

impl From<CopyMoveOutput> for ToolOutput {
    fn from(o: CopyMoveOutput) -> Self {
        ToolOutput::Text(TextOutput::from(o.message))
    }
}

fn resolve_paths(
    cwd: &Path,
    display_cwd: Option<&Path>,
    input: &CopyMoveInput,
) -> (PathBuf, PathBuf) {
    (
        resolve_model_path(cwd, display_cwd, &input.source),
        resolve_model_path(cwd, display_cwd, &input.destination),
    )
}

fn ensure_parent(dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create parent directory {}: {e}",
                parent.display()
            )
        })?;
    }
    Ok(())
}

fn dest_exists_error(dest: &Path, overwrite: bool) -> Option<String> {
    if dest.exists() && !overwrite {
        Some(format!(
            "destination {} already exists; set overwrite=true to replace it, or choose a new path",
            dest.display()
        ))
    } else {
        None
    }
}

fn is_tracked(git_root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(git_root).unwrap_or(path);
    Command::new("git")
        .args([
            "--no-optional-locks",
            "ls-files",
            "--error-unmatch",
            "--",
            &rel.to_string_lossy(),
        ])
        .current_dir(git_root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_mv(git_root: &Path, source: &Path, dest: &Path) -> Result<(), String> {
    let src_rel = source.strip_prefix(git_root).unwrap_or(source);
    let dest_rel = dest.strip_prefix(git_root).unwrap_or(dest);
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "mv",
            "--",
            &src_rel.to_string_lossy(),
            &dest_rel.to_string_lossy(),
        ])
        .current_dir(git_root)
        .output()
        .map_err(|e| format!("failed to spawn git mv: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git mv failed: {}", stderr.trim()))
    }
}

async fn notify_written(
    notification_handle: &crate::notification::types::ToolNotificationHandle,
    tool_call_id: String,
    dest: &Path,
    content: Vec<u8>,
    previous: Option<String>,
    is_new_file: bool,
) {
    notification_handle.send_file_written(FileWritten {
        tool_call_id,
        absolute_path: dest.to_path_buf(),
        content: String::from_utf8_lossy(&content).into_owned(),
        previous_content: previous,
        is_new_file,
    });
}

/// Copy a file from `source` to `destination`.
#[derive(Debug, Default)]
pub struct CopyFileTool;

impl ToolMetadata for CopyFileTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Move
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        COPY_DESCRIPTION
    }

    fn emitted_notifications(&self) -> &'static [&'static str] {
        &["FileWritten"]
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for CopyFileTool {
    type Args = CopyMoveInput;
    type Output = CopyMoveOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new("copy_file").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "copy_file",
            ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.copy_file", skip_all, fields(source = %input.source, destination = %input.destination))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: CopyMoveInput,
    ) -> Result<CopyMoveOutput, xai_tool_runtime::ToolError> {
        run_copy_or_move(&ctx, input, CopyMoveKind::Copy).await
    }
}

/// Move or rename a file, preferring `git mv` when the source is tracked.
#[derive(Debug, Default)]
pub struct MoveFileTool;

impl ToolMetadata for MoveFileTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Move
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        MOVE_DESCRIPTION
    }

    fn emitted_notifications(&self) -> &'static [&'static str] {
        &["FileWritten"]
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MoveFileTool {
    type Args = CopyMoveInput;
    type Output = CopyMoveOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new("move_file").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "move_file",
            ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.move_file", skip_all, fields(source = %input.source, destination = %input.destination))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: CopyMoveInput,
    ) -> Result<CopyMoveOutput, xai_tool_runtime::ToolError> {
        run_copy_or_move(&ctx, input, CopyMoveKind::Move).await
    }
}

#[derive(Clone, Copy)]
enum CopyMoveKind {
    Copy,
    Move,
}

async fn run_copy_or_move(
    ctx: &xai_tool_runtime::ToolCallContext,
    input: CopyMoveInput,
    kind: CopyMoveKind,
) -> Result<CopyMoveOutput, xai_tool_runtime::ToolError> {
    use crate::types::tool_metadata::shared_resources;
    let resources = shared_resources(ctx)?;
    let (cwd, display_cwd, fs, notification_handle) = {
        let cwd = crate::types::tool_metadata::resolve_cwd(ctx, &resources).await?;
        let res = resources.lock().await;
        let display_cwd = res.get::<DisplayCwd>().map(|d| d.0.clone());
        let fs = res.require::<FileSystem>()?.0.clone();
        let notification_handle = res.require::<NotificationHandle>()?.0.clone();
        (cwd, display_cwd, fs, notification_handle)
    };
    let (source, dest) = resolve_paths(&cwd, display_cwd.as_deref(), &input);
    if source == dest {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(
            "source and destination are the same path",
        ));
    }
    let src_meta = std::fs::metadata(&source).map_err(|e| {
        xai_tool_runtime::ToolError::invalid_arguments(format!(
            "source {} does not exist: {e}",
            source.display()
        ))
    })?;
    if src_meta.is_dir() {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
            "{} is a directory; this tool copies/moves files. Use `git mv` or `cp -r` via the shell for a tree.",
            source.display()
        )));
    }
    if let Some(msg) = dest_exists_error(&dest, input.overwrite) {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(msg));
    }
    ensure_parent(&dest).map_err(xai_tool_runtime::ToolError::invalid_arguments)?;

    let previous = match fs.read_file(&dest).await {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(_) => None,
    };
    let is_new_file = previous.is_none();
    let content = fs.read_file(&source).await.map_err(|e| {
        xai_tool_runtime::ToolError::execution(
            xai_tool_protocol::ToolId::new("copy_file").expect("valid"),
            e.to_string(),
        )
    })?;

    match kind {
        CopyMoveKind::Copy => {
            fs.write_file(&dest, &content).await.map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("copy_file").expect("valid"),
                    e.to_string(),
                )
            })?;
        }
        CopyMoveKind::Move => {
            let git_root = crate::implementations::editor_infra::duplicate_write::find_git_root(
                &cwd,
            )
            .or_else(|| {
                crate::implementations::editor_infra::duplicate_write::find_git_root(&source)
            });
            let used_git_mv = if let Some(root) = git_root.as_deref() {
                if is_tracked(root, &source) {
                    match git_mv(root, &source, &dest) {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::debug!(error = %e, "git mv failed; falling back to rename");
                            false
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !used_git_mv {
                if let Err(e) = std::fs::rename(&source, &dest) {
                    fs.write_file(&dest, &content).await.map_err(|e| {
                        xai_tool_runtime::ToolError::execution(
                            xai_tool_protocol::ToolId::new("move_file").expect("valid"),
                            e.to_string(),
                        )
                    })?;
                    let _ = fs.delete_file(&source).await;
                    let _ = e; // EXDEV fallback: copy-then-delete already applied.
                }
            }
        }
    }

    notify_written(
        &notification_handle,
        ctx.call_id.as_str().to_owned(),
        &dest,
        content,
        previous,
        is_new_file,
    )
    .await;

    let verb = match kind {
        CopyMoveKind::Copy => "Copied",
        CopyMoveKind::Move => "Moved",
    };
    Ok(CopyMoveOutput {
        message: format!("{verb} {} -> {}.", source.display(), dest.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::local::LocalFs;
    use crate::notification::types::ToolNotificationHandle;
    use crate::types::resources::{Cwd, Resources};
    use crate::types::tool_metadata::test_ctx;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_resources(cwd: &std::path::Path) -> Resources {
        let mut resources = Resources::new();
        resources.insert(Cwd(cwd.to_path_buf()));
        resources.insert(FileSystem(Arc::new(LocalFs)));
        resources.insert(NotificationHandle(ToolNotificationHandle::noop()));
        resources
    }

    #[tokio::test]
    async fn copy_file_duplicates_content() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("a.rs");
        std::fs::write(&src, "fn a() {}\n").unwrap();
        let tool = CopyFileTool;
        let input = CopyMoveInput {
            source: src.to_string_lossy().into_owned(),
            destination: tmp.path().join("b.rs").to_string_lossy().into_owned(),
            overwrite: false,
        };
        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(test_resources(tmp.path()).into_shared()), input)
            .await
            .unwrap();
        assert!(result.message.contains("Copied"), "{}", result.message);
        assert_eq!(std::fs::read_to_string(tmp.path().join("b.rs")).unwrap(), "fn a() {}\n");
        assert!(src.exists());
    }

    #[tokio::test]
    async fn copy_refuses_existing_dest_without_overwrite() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "a\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "b\n").unwrap();
        let tool = CopyFileTool;
        let input = CopyMoveInput {
            source: tmp.path().join("a.rs").to_string_lossy().into_owned(),
            destination: tmp.path().join("b.rs").to_string_lossy().into_owned(),
            overwrite: false,
        };
        let err = xai_tool_runtime::Tool::run(&tool, test_ctx(test_resources(tmp.path()).into_shared()), input)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[tokio::test]
    async fn move_file_renames() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("old.rs");
        std::fs::write(&src, "fn old() {}\n").unwrap();
        let dest = tmp.path().join("new.rs");
        let tool = MoveFileTool;
        let input = CopyMoveInput {
            source: src.to_string_lossy().into_owned(),
            destination: dest.to_string_lossy().into_owned(),
            overwrite: false,
        };
        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(test_resources(tmp.path()).into_shared()), input)
            .await
            .unwrap();
        assert!(result.message.contains("Moved"), "{}", result.message);
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "fn old() {}\n");
    }

    #[tokio::test]
    async fn move_tracked_file_uses_git_mv() {
        let tmp = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "t@e.com"])
            .current_dir(tmp.path())
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(tmp.path())
            .status();
        let src = tmp.path().join("old.rs");
        std::fs::write(&src, "fn old() {}\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "old.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "add"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let dest = tmp.path().join("renamed.rs");
        let tool = MoveFileTool;
        let input = CopyMoveInput {
            source: src.to_string_lossy().into_owned(),
            destination: dest.to_string_lossy().into_owned(),
            overwrite: false,
        };
        xai_tool_runtime::Tool::run(&tool, test_ctx(test_resources(tmp.path()).into_shared()), input)
            .await
            .unwrap();
        assert!(!src.exists());
        assert!(dest.exists());
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let porcelain = String::from_utf8_lossy(&status.stdout);
        assert!(
            porcelain.contains("renamed.rs") || porcelain.contains("R  old.rs"),
            "expected git to record the rename, got:\n{porcelain}"
        );
    }

    #[test]
    fn descriptions_ban_rewrite() {
        assert!(COPY_DESCRIPTION.contains("Do not retype"));
        assert!(MOVE_DESCRIPTION.contains("git mv"));
    }
}
