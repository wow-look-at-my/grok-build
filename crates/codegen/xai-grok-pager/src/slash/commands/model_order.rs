//! Order and visibility of the `/model` list.

use std::cmp::Ordering;

/// A batch route takes queued jobs, so it cannot answer an interactive turn.
pub(super) fn is_batch_model(id: &str) -> bool {
    id.ends_with(":batch")
}

/// Orders model names so a family's newest version comes first. Text compares
/// ascending and each run of digits compares numerically, descending.
pub(super) fn newest_first(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a.as_bytes()
        .chunk_by(same_kind)
        .zip(b.as_bytes().chunk_by(same_kind))
        .map(|(left, right)| compare_chunks(left, right))
        .find(|order| order.is_ne())
        .unwrap_or_else(|| a.len().cmp(&b.len()))
}

fn same_kind(a: &u8, b: &u8) -> bool {
    a.is_ascii_digit() == b.is_ascii_digit()
}

fn compare_chunks(left: &[u8], right: &[u8]) -> Ordering {
    let digits = |chunk: &[u8]| chunk.first().is_some_and(u8::is_ascii_digit);
    if !digits(left) || !digits(right) {
        return left.cmp(right);
    }
    let (left, right) = (significant(left), significant(right));
    right.len().cmp(&left.len()).then_with(|| right.cmp(left))
}

fn significant(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|d| **d == b'0').count();
    &digits[zeros..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted<'a>(mut names: Vec<&'a str>) -> Vec<&'a str> {
        names.sort_by(|a, b| newest_first(a, b));
        names
    }

    #[test]
    fn a_newer_version_sorts_first() {
        assert_eq!(
            sorted(vec![
                "Claude Opus 4.1",
                "Claude Opus 5.5",
                "Claude Opus 4.5"
            ]),
            vec!["Claude Opus 5.5", "Claude Opus 4.5", "Claude Opus 4.1"]
        );
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(
            sorted(vec!["Model 4.9", "Model 4.10", "Model 4.09"]),
            vec!["Model 4.10", "Model 4.9", "Model 4.09"]
        );
    }

    #[test]
    fn families_stay_alphabetical() {
        assert_eq!(
            sorted(vec!["Sonnet 5.5", "opus 4.1", "Haiku 5.5"]),
            vec!["Haiku 5.5", "opus 4.1", "Sonnet 5.5"]
        );
    }

    #[test]
    fn a_digit_run_longer_than_any_integer_still_compares() {
        let huge = "9".repeat(64);
        let name = format!("Model {huge}");
        assert_eq!(
            sorted(vec!["Model 1", &name]),
            vec![name.as_str(), "Model 1"]
        );
    }

    #[test]
    fn only_a_batch_route_is_a_batch_model() {
        assert!(is_batch_model("openrouter/anthropic/claude-opus-5.5:batch"));
        assert!(!is_batch_model("openrouter/anthropic/claude-opus-5.5"));
    }
}
