//! The bearer fragment every 401-attribution site compares.

/// Trailing characters of a bearer that may cross an attribution boundary.
pub const BEARER_SUFFIX_LEN: usize = 12;

/// Last [`BEARER_SUFFIX_LEN`] characters, or the whole string if shorter.
pub fn bearer_suffix(s: &str) -> &str {
    match s.char_indices().rev().nth(BEARER_SUFFIX_LEN - 1) {
        Some((i, _)) => s.get(i..).unwrap_or(s),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_suffix_semantics() {
        for (input, expected) in [
            // The tail, not the head: JWT headers and xAI key prefixes are shared.
            ("eyJ0eXAiOiJh.shared-head.tail-distinct", "ail-distinct"),
            ("xai-key-aaaaaaaaaaadistinct1", "aaadistinct1"),
            // Shorter than the fragment: returned whole.
            ("abc", "abc"),
            ("", ""),
            ("123456789012", "123456789012"),
            ("éabcdefghijk", "éabcdefghijk"),
            ("ééééééééééééé", "éééééééééééé"),
            ("🔑🔑🔑🔑🔑🔑🔑", "🔑🔑🔑🔑🔑🔑🔑"),
        ] {
            assert_eq!(bearer_suffix(input), expected, "input={input:?}");
        }
    }
}
