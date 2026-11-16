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
) -> Result<PreparedEditorRequest, PrepareError> {
    let argv = editor_argv().map_err(|message| PrepareError { message })?;
    match request {
        PendingEditorRequest::ConfigFile {
            path,
            refresh_agents_modal,
        } => Ok(PreparedEditorRequest::ConfigFile {
            launch: EditorLaunch { argv, path },
            refresh_agents_modal,
        }),
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

    fn get_agent(
        app: &AppView,
        id: crate::app::agent::AgentId,
    ) -> &crate::app::agent_view::AgentView {
        let Some(a) = app.agents.get(&id) else {
            panic!("missing agent {id:?}");
        };
        a
    }

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
            get_agent(&app, id)
                .toast
                .as_ref()
                .map(|(text, _)| text.as_str()),
            Some(message)
        );
<<<<<<< fork
||||||| base

        app.agents.get_mut(&id).unwrap().toast = None;
        app.screen_mode = crate::app::ScreenMode::Minimal;
        finish_prepare_error(&mut app, error());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(message))
        );
    }

    #[test]
    fn revalidation_cancels_after_warm_voice_ownership_change() {
        let (mut app, request) = app_with_prompt_request();
        let id = AgentId(0);
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        app.voice_cmd_tx = Some(tx);
        app.voice_state = crate::app::app_view::VoiceState::Recording {
            hold: false,
            target: crate::app::app_view::VoiceTarget::Agent(id),
            interim: None,
        };
        assert!(prepare(&mut app, request).unwrap().is_none());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(VOICE_MESSAGE))
        );
    }

    #[test]
    fn revalidation_cancels_after_cold_voice_ownership_change() {
        let (mut app, request) = app_with_prompt_request();
        let id = AgentId(0);
        app.voice_state = crate::app::app_view::VoiceState::ColdStart {
            hold: false,
            target: crate::app::app_view::VoiceTarget::Agent(id),
        };
        assert!(prepare(&mut app, request).unwrap().is_none());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(VOICE_MESSAGE))
        );
    }

    #[test]
    fn revalidation_cancels_after_paste_probe_ownership_change() {
        let (mut app, request) = app_with_prompt_request();
        let id = AgentId(0);
        app.agents.get_mut(&id).unwrap().paste_probe_in_flight = 1;
        assert!(prepare(&mut app, request).unwrap().is_none());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(PASTE_MESSAGE))
        );
    }

    #[test]
    fn finish_success_failure_and_stale_paths_preserve_contracts() {
        let id = AgentId(0);
        let mut app = crate::app::app_view::tests::test_app();
        let mut agent = crate::test_util::make_agent_view(Some("session"), "/work");
        agent.session.id = id;
        agent.prompt.set_text("original");
        app.agents.insert(id, agent);
        app.active_view = ActiveView::Agent(id);

        let file = PromptEditorFile::create("edited\n").unwrap();
        let prepared = PreparedEditorRequest::PromptDraft {
            launch: EditorLaunch {
                argv: vec!["editor".to_owned()],
                path: file.path().to_path_buf(),
            },
            agent_id: id,
            original_text: "original".to_owned(),
            file,
        };
        finish(&mut app, prepared, Ok(success_status()));
        assert_eq!(
            get_agent(&app, id).prompt.text(),
            "edited",
            "the editor's final newline is stripped"
        );

        app.agents.get_mut(&id).unwrap().prompt.set_text("newer");
        apply_prompt_outcome(&mut app, id, "original".to_owned(), Ok("stale".to_owned()));
        assert_eq!(get_agent(&app, id).prompt.text(), "newer");

        apply_prompt_outcome(&mut app, id, "newer".to_owned(), Err(PROMPT_EDITOR_NONZERO));
        assert_eq!(get_agent(&app, id).prompt.text(), "newer");
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref()
                    == Some(PROMPT_EDITOR_NONZERO))
        );
    }

    /// The trailing-newline strip tells an editor-added newline from one the draft already had.
    /// Editors append a final newline only when the buffer doesn't already end with one.
    /// So a draft that ended with '\n' keeps it, and a blank line the user adds to a newline-less draft survives as exactly one '\n'.
    #[test]
    fn finish_preserves_a_drafts_own_trailing_newline() {
        let id = AgentId(0);
        let mut app = crate::app::app_view::tests::test_app();
        let mut agent = crate::test_util::make_agent_view(Some("session"), "/work");
        agent.session.id = id;
        app.agents.insert(id, agent);
        app.active_view = ActiveView::Agent(id);

        // The draft ends with '\n': the file was saved as-is, so nothing is stripped
        app.agents.get_mut(&id).unwrap().prompt.set_text("line\n");
        let file = PromptEditorFile::create("edited\n").unwrap();
        let prepared = PreparedEditorRequest::PromptDraft {
            launch: EditorLaunch {
                argv: vec!["editor".to_owned()],
                path: file.path().to_path_buf(),
            },
            agent_id: id,
            original_text: "line\n".to_owned(),
            file,
        };
        finish(&mut app, prepared, Ok(success_status()));
        assert_eq!(
            get_agent(&app, id).prompt.text(),
            "edited\n",
            "a draft's own trailing newline survives the round trip"
        );

        // Newline-less draft, the user appends a blank line: the editor saved "text\n\n"; only the editor-added final '\n' is stripped
        app.agents.get_mut(&id).unwrap().prompt.set_text("text");
        let file = PromptEditorFile::create("text\n\n").unwrap();
        let prepared = PreparedEditorRequest::PromptDraft {
            launch: EditorLaunch {
                argv: vec!["editor".to_owned()],
                path: file.path().to_path_buf(),
            },
            agent_id: id,
            original_text: "text".to_owned(),
            file,
        };
        finish(&mut app, prepared, Ok(success_status()));
        assert_eq!(
            get_agent(&app, id).prompt.text(),
            "text\n",
            "a deliberately added trailing blank line survives as one newline"
        );
    }

    #[test]
    fn unconsumed_prompt_request_has_no_materialized_path() {
        let request = PendingEditorRequest::PromptDraft {
            agent_id: AgentId(7),
            original_text: "sensitive draft".to_owned(),
        };
        assert!(matches!(
            request,
            PendingEditorRequest::PromptDraft {
                agent_id: AgentId(7),
                ref original_text,
            } if original_text == "sensitive draft"
        ));
        drop(request);
=======

        app.agents.get_mut(&id).unwrap().toast = None;
        app.screen_mode = crate::app::ScreenMode::Minimal;
        finish_prepare_error(&mut app, error());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(message))
        );
    }

    #[test]
    fn revalidation_cancels_after_warm_voice_ownership_change() {
        let (mut app, request) = app_with_prompt_request();
        let id = AgentId(0);
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        app.voice_cmd_tx = Some(tx);
        app.voice_state = crate::app::app_view::VoiceState::Recording {
            hold: false,
            target: crate::app::app_view::VoiceTarget::Agent(id),
            partial: crate::app::app_view::Partial::None,
            route: None,
        };
        assert!(prepare(&mut app, request).unwrap().is_none());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(VOICE_MESSAGE))
        );
    }

    #[test]
    fn revalidation_cancels_after_cold_voice_ownership_change() {
        let (mut app, request) = app_with_prompt_request();
        let id = AgentId(0);
        app.voice_state = crate::app::app_view::VoiceState::ColdStart {
            hold: false,
            target: crate::app::app_view::VoiceTarget::Agent(id),
        };
        assert!(prepare(&mut app, request).unwrap().is_none());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(VOICE_MESSAGE))
        );
    }

    #[test]
    fn revalidation_cancels_after_paste_probe_ownership_change() {
        let (mut app, request) = app_with_prompt_request();
        let id = AgentId(0);
        app.agents.get_mut(&id).unwrap().paste_probe_in_flight = 1;
        assert!(prepare(&mut app, request).unwrap().is_none());
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref() == Some(PASTE_MESSAGE))
        );
    }

    #[test]
    fn finish_success_failure_and_stale_paths_preserve_contracts() {
        let id = AgentId(0);
        let mut app = crate::app::app_view::tests::test_app();
        let mut agent = crate::test_util::make_agent_view(Some("session"), "/work");
        agent.session.id = id;
        agent.prompt.set_text("original");
        app.agents.insert(id, agent);
        app.active_view = ActiveView::Agent(id);

        let file = PromptEditorFile::create("edited\n").unwrap();
        let prepared = PreparedEditorRequest::PromptDraft {
            launch: EditorLaunch {
                argv: vec!["editor".to_owned()],
                path: file.path().to_path_buf(),
            },
            agent_id: id,
            original_text: "original".to_owned(),
            file,
        };
        finish(&mut app, prepared, Ok(success_status()));
        assert_eq!(
            get_agent(&app, id).prompt.text(),
            "edited",
            "the editor's final newline is stripped"
        );

        app.agents.get_mut(&id).unwrap().prompt.set_text("newer");
        apply_prompt_outcome(&mut app, id, "original".to_owned(), Ok("stale".to_owned()));
        assert_eq!(get_agent(&app, id).prompt.text(), "newer");

        apply_prompt_outcome(&mut app, id, "newer".to_owned(), Err(PROMPT_EDITOR_NONZERO));
        assert_eq!(get_agent(&app, id).prompt.text(), "newer");
        assert!(
            get_agent(&app, id)
                .scrollback
                .iter_entries()
                .any(|(_, entry)| entry.block.searchable_text().as_deref()
                    == Some(PROMPT_EDITOR_NONZERO))
        );
    }

    /// The trailing-newline strip tells an editor-added newline from one the draft already had.
    /// Editors append a final newline only when the buffer doesn't already end with one.
    /// So a draft that ended with '\n' keeps it, and a blank line the user adds to a newline-less draft survives as exactly one '\n'.
    #[test]
    fn finish_preserves_a_drafts_own_trailing_newline() {
        let id = AgentId(0);
        let mut app = crate::app::app_view::tests::test_app();
        let mut agent = crate::test_util::make_agent_view(Some("session"), "/work");
        agent.session.id = id;
        app.agents.insert(id, agent);
        app.active_view = ActiveView::Agent(id);

        // The draft ends with '\n': the file was saved as-is, so nothing is stripped
        app.agents.get_mut(&id).unwrap().prompt.set_text("line\n");
        let file = PromptEditorFile::create("edited\n").unwrap();
        let prepared = PreparedEditorRequest::PromptDraft {
            launch: EditorLaunch {
                argv: vec!["editor".to_owned()],
                path: file.path().to_path_buf(),
            },
            agent_id: id,
            original_text: "line\n".to_owned(),
            file,
        };
        finish(&mut app, prepared, Ok(success_status()));
        assert_eq!(
            get_agent(&app, id).prompt.text(),
            "edited\n",
            "a draft's own trailing newline survives the round trip"
        );

        // Newline-less draft, the user appends a blank line: the editor saved "text\n\n"; only the editor-added final '\n' is stripped
        app.agents.get_mut(&id).unwrap().prompt.set_text("text");
        let file = PromptEditorFile::create("text\n\n").unwrap();
        let prepared = PreparedEditorRequest::PromptDraft {
            launch: EditorLaunch {
                argv: vec!["editor".to_owned()],
                path: file.path().to_path_buf(),
            },
            agent_id: id,
            original_text: "text".to_owned(),
            file,
        };
        finish(&mut app, prepared, Ok(success_status()));
        assert_eq!(
            get_agent(&app, id).prompt.text(),
            "text\n",
            "a deliberately added trailing blank line survives as one newline"
        );
    }

    #[test]
    fn unconsumed_prompt_request_has_no_materialized_path() {
        let request = PendingEditorRequest::PromptDraft {
            agent_id: AgentId(7),
            original_text: "sensitive draft".to_owned(),
        };
        assert!(matches!(
            request,
            PendingEditorRequest::PromptDraft {
                agent_id: AgentId(7),
                ref original_text,
            } if original_text == "sensitive draft"
        ));
        drop(request);
>>>>>>> upstream
    }
}
