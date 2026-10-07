#![allow(clippy::string_slice)]

//! Shared prompt-queue wire types, merge rules, and the definition of "this queue row's text is a command".

#![deny(clippy::indexing_slicing)]

mod combine;
mod types;

pub use combine::{
    CombineGate, TEXT_SEPARATOR, can_merge_follower, can_merge_front, combine_prefix_len,
    is_combined, join_texts, stamp_combined_display_texts,
};
pub use types::{COMBINED_DISPLAY_TEXTS_META, QueueChanged, QueueEntryMeta, QueueEntryWire};

/// Whether `text` is a slash invocation — a `/name` or `/name args` line.
pub fn is_slash_invocation(text: &str) -> bool {
    let trimmed = text.trim();
    let Some(without_slash) = trimmed.strip_prefix('/') else {
        return false;
    };
    let name = without_slash
        .find(char::is_whitespace)
        .map_or(without_slash, |idx| {
            // `idx` is the byte offset of a whitespace character, hence a boundary.
            #[allow(clippy::string_slice)] // up to a matched whitespace character
            let before_space = &without_slash[..idx];
            before_space
        });
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
