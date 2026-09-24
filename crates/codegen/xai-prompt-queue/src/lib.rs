//! Shared prompt-queue wire types, merge rules, and the one definition of
//! "this queue row's text is a command".

#![deny(clippy::indexing_slicing)]

mod combine;
mod types;

pub use combine::{
    CombineGate, TEXT_SEPARATOR, can_merge_follower, can_merge_front, combine_prefix_len,
    is_combined, join_texts, stamp_combined_display_texts,
};
pub use types::{COMBINED_DISPLAY_TEXTS_META, QueueChanged, QueueEntryMeta, QueueEntryWire};

/// Whether `text` is a slash invocation — a `/name` or `/name args` line.
///
/// A command line means something only as the LEADING token of its own turn:
/// the shell resolves a prompt's first token, so a row carrying one must never
/// be delivered as ordinary user text by any other route (folded into a running
/// turn, merged with a neighbour, migrated onto the server queue) or the model
/// reads the literal `/cmd args` and the command never runs.
///
/// This is the SINGLE definition of that shape. Both ends — the pager
/// (`QueuedPrompt` classification and the submit path) and the shell
/// (`deliverable_mid_turn`, the promote-path combine gate, the goal merges) —
/// ask here, so the two cannot drift into disagreeing about what a command is.
/// A bare `/` or a lone `/ ` is not one: no token, so the line is ordinary text.
pub fn is_slash_invocation(text: &str) -> bool {
    let trimmed = text.trim();
    let Some(without_slash) = trimmed.strip_prefix('/') else {
        return false;
    };
    let name = without_slash
        .find(char::is_whitespace)
        .map_or(without_slash, |idx| &without_slash[..idx]);
    !name.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_invocations_are_recognised() {
        for text in [
            "/plan",
            "/plan implement the auth flow",
            "/compact keep the auth notes",
            "  /model grok-4  ",
            "/TODO jump the queue",
            "//",
        ] {
            assert!(is_slash_invocation(text), "{text:?} is a command line");
        }
    }

    #[test]
    fn ordinary_text_is_not_a_slash_invocation() {
        for text in [
            "",
            "   ",
            "/",
            "/ ",
            "/\t",
            "hello",
            "hello /plan implement it",
            "!ls -la",
            "https://example.com/x",
        ] {
            assert!(
                !is_slash_invocation(text),
                "{text:?} is ordinary text and must be deliverable as a prompt"
            );
        }
    }

    /// The shape the pager's submit path treats as a command and the shape this
    /// predicate accepts must be the same line.
    #[test]
    fn classification_matches_a_leading_slash_token() {
        assert!(is_slash_invocation("/gboom guide me"));
        assert!(is_slash_invocation("/scroll-debug"));
        assert!(!is_slash_invocation("please run /commit"));
        assert!(!is_slash_invocation("/ \n"));
    }
}
