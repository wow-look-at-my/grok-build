//! Detects whether grok is running inside an editor's embedded `:terminal` (Neovim/Vim `:terminal`, Emacs `vterm`).

use std::collections::HashMap;

use super::env_get;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedEditor {
    /// Neovim `:terminal` (sets `NVIM`, or legacy `NVIM_LISTEN_ADDRESS`).
    Neovim,
    Vim,
    /// Emacs (sets `INSIDE_EMACS`; `vterm` uses libvterm, same bug).
    Emacs,
}

/// Empty values are absent. A new marker must be added to the PTY harness strip list so the host terminal cannot leak into tests.
pub fn embedded_editor_from_env(env: &HashMap<String, String>) -> Option<EmbeddedEditor> {
    // The markers cannot tell the editor running inside tmux (where wrapping
    // always renders garbage).
    if env_get(env, "NVIM").is_some() || env_get(env, "NVIM_LISTEN_ADDRESS").is_some() {
        return Some(EmbeddedEditor::Neovim);
    }
    if env_get(env, "VIM_TERMINAL").is_some() {
        return Some(EmbeddedEditor::Vim);
    }
    if env_get(env, "INSIDE_EMACS").is_some() {
        return Some(EmbeddedEditor::Emacs);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::env_from;

    #[test]
    fn nvim_detected_as_neovim() {
        let env = env_from(&[("NVIM", "/tmp/nvim.12345.0")]);
        assert_eq!(embedded_editor_from_env(&env), Some(EmbeddedEditor::Neovim));
    }

    #[test]
    fn nvim_listen_address_detected_as_neovim() {
        let env = env_from(&[("NVIM_LISTEN_ADDRESS", "/tmp/nvim.sock")]);
        assert_eq!(embedded_editor_from_env(&env), Some(EmbeddedEditor::Neovim));
    }

    #[test]
    fn vim_terminal_detected_as_vim() {
        let env = env_from(&[("VIM_TERMINAL", "8.2")]);
        assert_eq!(embedded_editor_from_env(&env), Some(EmbeddedEditor::Vim));
    }

    #[test]
    fn inside_emacs_detected_as_emacs() {
        let env = env_from(&[("INSIDE_EMACS", "30.1,vterm")]);
        assert_eq!(embedded_editor_from_env(&env), Some(EmbeddedEditor::Emacs));
    }

    #[test]
    fn no_editor_markers_is_none() {
        let env = env_from(&[
            ("TERM", "xterm-256color"),
            ("TMUX", "/tmp/tmux-501/default,1,0"),
        ]);
        assert_eq!(embedded_editor_from_env(&env), None);
    }

    #[test]
    fn empty_value_treated_as_absent() {
        let env = env_from(&[("NVIM", ""), ("VIM_TERMINAL", ""), ("INSIDE_EMACS", "")]);
        assert_eq!(embedded_editor_from_env(&env), None);
    }

    #[test]
    fn nvim_beats_vim_and_emacs() {
        let env = env_from(&[
            ("NVIM", "/tmp/nvim.12345.0"),
            ("VIM_TERMINAL", "8.2"),
            ("INSIDE_EMACS", "30.1,vterm"),
        ]);
        assert_eq!(embedded_editor_from_env(&env), Some(EmbeddedEditor::Neovim));
    }
}
