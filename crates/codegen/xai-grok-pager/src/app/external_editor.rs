//! External-editor request lifecycle for configuration files.

use std::path::PathBuf;

use crate::app::app_view::{ActiveView, AppView};

#[derive(Clone, Debug)]
pub(crate) enum PendingEditorRequest {
    /// Edit an agents/personas configuration file, then refresh its modal tab.
    ConfigFile {
        path: PathBuf,
        refresh_agents_modal: Option<crate::views::agents_modal::AgentsTab>,
    },
}

pub(crate) struct EditorLaunch {
    pub(crate) argv: Vec<String>,
    pub(crate) path: PathBuf,
}

pub(crate) enum PreparedEditorRequest {
    ConfigFile {
        launch: EditorLaunch,
        refresh_agents_modal: Option<crate::views::agents_modal::AgentsTab>,
    },
}

impl PreparedEditorRequest {
    pub(crate) fn launch(&self) -> &EditorLaunch {
        match self {
            Self::ConfigFile { launch, .. } => launch,
        }
    }
}

fn parse_editor_argv(command: &str) -> Result<Vec<String>, String> {
    shlex::split(command)
        .filter(|parts| parts.first().is_some_and(|program| !program.is_empty()))
        .ok_or_else(|| "could not parse $VISUAL or $EDITOR".to_owned())
}

fn resolve_editor_argv(visual: Option<&str>, editor: Option<&str>) -> Result<Vec<String>, String> {
    let command = visual
        .filter(|value| !value.trim().is_empty())
        .or_else(|| editor.filter(|value| !value.trim().is_empty()))
        .unwrap_or("vi");
    parse_editor_argv(command)
}

fn editor_argv() -> Result<Vec<String>, String> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    resolve_editor_argv(visual.as_deref(), editor.as_deref())
}

pub(crate) fn prepare(
    request: PendingEditorRequest,
) -> Result<Option<PreparedEditorRequest>, PrepareError> {
    let argv = editor_argv().map_err(|message| PrepareError { message })?;
    match request {
        PendingEditorRequest::ConfigFile {
            path,
            refresh_agents_modal,
        } => Ok(Some(PreparedEditorRequest::ConfigFile {
            launch: EditorLaunch { argv, path },
            refresh_agents_modal,
        })),
    }
}

#[derive(Debug)]
pub(crate) struct PrepareError {
    message: String,
}

pub(crate) fn finish_prepare_error(app: &mut AppView, error: PrepareError) {
    tracing::warn!(error = %error.message, "external editor: could not prepare child");
    app.show_toast(&error.message);
}

pub(crate) fn finish(
    app: &mut AppView,
    prepared: PreparedEditorRequest,
    editor_result: Result<std::process::ExitStatus, std::io::Error>,
) {
    match prepared {
        PreparedEditorRequest::ConfigFile {
            refresh_agents_modal,
            ..
        } => {
            if let Err(error) = editor_result {
                tracing::warn!(%error, "configuration editor: child failed");
            }
            if let Some(tab) = refresh_agents_modal
                && let ActiveView::Agent(id) = app.active_view
                && let Some(agent) = app.agents.get_mut(&id)
                && let Some(ref mut modal) = agent.agents_modal
            {
                modal.refresh_after_editor(tab);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::AgentId;

    #[test]
    fn editor_resolution_and_parsing_follow_visual_editor_vi_order() {
        assert_eq!(
            resolve_editor_argv(Some("visual --wait"), Some("editor")).unwrap(),
            ["visual", "--wait"]
        );
        assert_eq!(
            resolve_editor_argv(Some("  "), Some("editor --name 'prompt draft'")).unwrap(),
            ["editor", "--name", "prompt draft"]
        );
        assert_eq!(resolve_editor_argv(None, None).unwrap(), ["vi"]);
        assert!(parse_editor_argv("editor 'unterminated").is_err());
        assert!(parse_editor_argv("   ").is_err());
    }

    #[test]
    fn config_prepare_error_is_shown_as_a_toast() {
        let id = AgentId(0);
        let mut app = crate::app::app_view::tests::test_app();
        app.agents.insert(
            id,
            crate::test_util::make_agent_view(Some("session"), "/work"),
        );
        app.active_view = ActiveView::Agent(id);
        let message = "could not parse $VISUAL or $EDITOR";
        finish_prepare_error(
            &mut app,
            PrepareError {
                message: message.to_owned(),
            },
        );
        assert_eq!(
            app.agents[&id]
                .toast
                .as_ref()
                .map(|(text, _)| text.as_str()),
            Some(message)
        );
    }
}
