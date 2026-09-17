//! What we have told one server about each open document.
//!
//! Two readers besides the client need this. Incremental servers require a
//! range on every change event, which is computed from where the previous
//! revision ended. And every diagnostic answer has to be attributed to a
//! document version — pull knows the version it asked about, and a pushed
//! report that omits `version` is credited with the newest version we had sent
//! when it arrived. Both of those happen off the client's thread, so the
//! versions live behind a shared handle rather than inside `LspClient`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_lsp::lsp_types::Position;

/// The version a document is opened at.
///
/// Deliberately above [`super::diagnostics::NO_VERSION`], which is what a
/// report about a document we have never opened is credited: were they equal,
/// such a report would count as a verdict on our first edit to that file.
pub const FIRST_VERSION: i32 = 1;
const _: () = assert!(FIRST_VERSION > super::diagnostics::NO_VERSION);

/// The revision of one document that the server has.
#[derive(Debug, Clone)]
pub struct Tracked {
    /// Version of the last notification we successfully sent for it.
    pub version: i32,
    pub language_id: String,
    /// Where that revision ends. Two integers, not a copy of the text.
    pub end: Position,
}

/// What the next notification for a document should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update {
    /// The server has never seen this document.
    Open { version: i32 },
    /// The server has it at `previous_end`; send `version` next.
    Change {
        version: i32,
        previous_end: Position,
    },
}

impl Update {
    pub fn version(self) -> i32 {
        match self {
            Update::Open { version } | Update::Change { version, .. } => version,
        }
    }
}

/// Open documents for one server connection.
///
/// Cheap to clone (shared handle). Lock poisoning is recovered from in one
/// place: a panicking writer leaves the map structurally intact, and a stale
/// version beats no version at all.
#[derive(Debug, Clone, Default)]
pub struct Documents {
    inner: Arc<RwLock<HashMap<String, Tracked>>>,
}

impl Documents {
    pub fn new() -> Self {
        Self::default()
    }

    /// What to send for `uri`, without recording it as sent.
    ///
    /// Deliberately separate from [`Self::commit`]: what is recorded there
    /// describes the text the *server* has, so a notification that failed to go
    /// out must not be left advanced — [`Self::restore`] takes it back.
    /// Leaving it advanced would aim every later incremental range at a
    /// revision the server never received — the same protocol violation the
    /// range exists to avoid.
    pub fn plan(&self, uri: &str) -> Update {
        match self.read().get(uri) {
            Some(tracked) => Update::Change {
                version: tracked.version.saturating_add(1),
                previous_end: tracked.end,
            },
            None => Update::Open {
                version: FIRST_VERSION,
            },
        }
    }

    /// A notification's revision is written down here, before the notification
    /// reaches the wire. The replaced value comes back, so [`Self::restore`]
    /// can undo a send that failed.
    ///
    /// A push that names no version is credited with the newest version we
    /// have sent, and the server can answer on another thread before the
    /// sending thread gets this far. A report read against the older record
    /// settles nothing, so the reader never sees it.
    pub fn commit(
        &self,
        uri: &str,
        version: i32,
        language_id: &str,
        end: Position,
    ) -> Option<Tracked> {
        let mut documents = self.write();
        // A document keeps the language it was opened with: renaming one the
        // server already has open is not what a later change means to say.
        let language_id = documents
            .get(uri)
            .map(|tracked| tracked.language_id.clone())
            .unwrap_or_else(|| language_id.to_string());
        documents.insert(
            uri.to_string(),
            Tracked {
                version,
                language_id,
                end,
            },
        )
    }

    /// Undo a [`Self::commit`] whose notification never went out.
    pub fn restore(&self, uri: &str, previous: Option<Tracked>) {
        let mut documents = self.write();
        match previous {
            Some(tracked) => documents.insert(uri.to_string(), tracked),
            None => documents.remove(uri),
        };
    }

    /// The version the server has, or `None` if it has never been told about
    /// this document.
    pub fn version(&self, uri: &str) -> Option<i32> {
        self.read().get(uri).map(|tracked| tracked.version)
    }

    pub fn contains(&self, uri: &str) -> bool {
        self.read().contains_key(uri)
    }

    /// Every open document, as `(uri, language_id)` — what a restart replays.
    pub fn tracked(&self) -> Vec<(String, String)> {
        self.read()
            .iter()
            .map(|(uri, tracked)| (uri.clone(), tracked.language_id.clone()))
            .collect()
    }

    /// Every open document's URI. A refresh re-pulls all of them.
    pub fn uris(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// Every open document with the version the server has, for re-asking
    /// questions a refresh has made open again.
    pub fn versions(&self) -> Vec<(String, i32)> {
        self.read()
            .iter()
            .map(|(uri, tracked)| (uri.clone(), tracked.version))
            .collect()
    }

    /// Forget everything, returning what was open so it can be closed.
    pub fn take_all(&self) -> Vec<String> {
        std::mem::take(&mut *self.write()).into_keys().collect()
    }

    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, Tracked>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, Tracked>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// End position of `text`, i.e. the position just past its final character.
pub fn end_position(text: &str) -> Position {
    // `lines()` drops a trailing newline, which would give a position that is
    // short of the real end of the document, so count explicitly.
    let mut line = 0u32;
    let mut last_line_start = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            line += 1;
            last_line_start = idx + 1;
        }
    }
    // LSP character offsets are UTF-16 code units.
    let character = text[last_line_start..].encode_utf16().count() as u32;
    Position { line, character }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "file:///a.cs";

    #[test]
    fn an_unknown_document_is_opened_above_the_no_version_marker() {
        let documents = Documents::new();
        assert_eq!(
            documents.plan(A),
            Update::Open {
                version: FIRST_VERSION
            }
        );
        assert_eq!(documents.version(A), None);
    }

    #[test]
    fn a_committed_document_is_changed_from_where_it_ended() {
        let documents = Documents::new();
        let previous = documents.commit(A, 1, "csharp", end_position("one\ntwo"));
        assert!(previous.is_none(), "nothing was open to replace");

        assert_eq!(
            documents.plan(A),
            Update::Change {
                version: 2,
                previous_end: Position {
                    line: 1,
                    character: 3
                },
            }
        );
        assert_eq!(documents.version(A), Some(1));
    }

    /// The plan is what to send; only a send that went out is committed. A
    /// failed one must leave the server's revision where it was, or the next
    /// incremental range will describe text the server never received.
    #[test]
    fn planning_alone_does_not_move_the_document() {
        let documents = Documents::new();
        documents.commit(A, 0, "csharp", end_position("one"));

        let planned = documents.plan(A);
        assert_eq!(
            documents.plan(A),
            planned,
            "planning twice is the same plan"
        );
        assert_eq!(documents.version(A), Some(0));
    }

    /// The revision is written down before the notification goes out, so a
    /// versionless push that arrives while the send is still running is
    /// credited with the text the server was just given. A send that fails
    /// takes it back: what is recorded describes the text the server has.
    #[test]
    fn a_failed_send_leaves_the_document_where_it_was() {
        let documents = Documents::new();
        documents.commit(A, 1, "csharp", end_position("one"));

        let previous = documents.commit(A, 2, "csharp", end_position("one\ntwo"));
        assert_eq!(documents.version(A), Some(2), "recorded before the send");

        documents.restore(A, previous);
        assert_eq!(documents.version(A), Some(1));
        assert_eq!(
            documents.plan(A),
            Update::Change {
                version: 2,
                previous_end: position(0, 3),
            },
            "the next change describes the text the server really has"
        );
    }

    /// The same, for a document the server has never been told about: there is
    /// nothing to put back, so the failed open leaves it unopened.
    #[test]
    fn a_failed_open_leaves_the_document_unopened() {
        let documents = Documents::new();
        let previous = documents.commit(A, FIRST_VERSION, "csharp", end_position("one"));

        documents.restore(A, previous);
        assert_eq!(documents.version(A), None);
        assert!(!documents.contains(A));
        assert_eq!(
            documents.plan(A),
            Update::Open {
                version: FIRST_VERSION
            },
            "the next attempt is an open, not a change"
        );
    }

    /// A document keeps the language it was opened with. A later change names
    /// whatever the caller resolved this time, and overwriting the recorded one
    /// would rename a document the server already has open.
    #[test]
    fn a_change_does_not_rename_the_document_language() {
        let documents = Documents::new();
        documents.commit(A, 1, "csharp", end_position("one"));
        documents.commit(A, 2, "typescript", end_position("one\ntwo"));

        assert_eq!(
            documents.tracked(),
            vec![(A.to_string(), "csharp".to_string())]
        );
    }

    fn position(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn end_position_counts_the_trailing_newline_as_a_new_line() {
        assert_eq!(end_position(""), position(0, 0));
        assert_eq!(end_position("abc"), position(0, 3));
        assert_eq!(end_position("abc\n"), position(1, 0));
        assert_eq!(end_position("const x = 1;\nabcde"), position(1, 5));
        assert_eq!(end_position("a\nb\nc\n"), position(3, 0));
    }

    #[test]
    fn end_position_measures_in_utf16_code_units() {
        // Astral-plane characters are two UTF-16 units; 'é' is one.
        assert_eq!(end_position("é"), position(0, 1));
        assert_eq!(end_position("🚀"), position(0, 2));
        assert_eq!(end_position("a\n🚀b"), position(1, 3));
    }

    #[test]
    fn take_all_empties_the_map() {
        let documents = Documents::new();
        documents.commit(A, FIRST_VERSION, "csharp", Position::default());
        assert_eq!(documents.take_all(), vec![A.to_string()]);
        assert!(documents.uris().is_empty());
        assert!(!documents.contains(A));
    }
}
