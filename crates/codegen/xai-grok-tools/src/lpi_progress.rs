//! Wrap a long-running command in `lpi` so a caller can read its progress.

use std::path::Path;

use serde::Deserialize;

/// One progress reading from the wrapped command.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Snapshot {
    /// Time-weighted progress, from zero to one.
    pub progress: f64,
    /// Reference occurrences matched so far.
    pub units_done: u64,
    /// Reference occurrences expected in a complete run.
    pub units_total: u64,
    /// Unit progress as a percentage.
    pub units_pct: f64,
    /// Whether the reference carries usable timing.
    pub has_times: bool,
    /// Elapsed time, or zero when unknown.
    pub elapsed_seconds: f64,
    /// Whether elapsed time is known.
    pub elapsed_known: bool,
    /// Seconds remaining. Absent when no estimate can be made.
    #[serde(default)]
    pub eta_seconds: Option<f64>,
    /// How the estimate was derived: `pace`, `ref-pace`, or `none`.
    pub eta_kind: String,
    /// Current speed against the reference, where above one is slower.
    pub pace: f64,
    /// Fraction of live lines matched.
    pub match_rate: f64,
    /// `high`, `medium`, `low`, or `none`.
    pub confidence: String,
    /// Live lines seen.
    pub current_lines: u64,
    /// Live lines that matched the reference.
    pub matched_lines: u64,
    /// Live lines the reference never carried.
    pub novel_lines: u64,
    /// Live lines seen more often than the reference expected.
    pub overflow_lines: u64,
}

/// Read one snapshot from a stderr line. The stream also carries `lpi`'s own
/// notices and usage text, which are not snapshots and yield nothing.
pub fn parse_snapshot(line: &str) -> Option<Snapshot> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// Whether `lpi` can be run.
pub fn available() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join("lpi").is_file())
}

/// A key `lpi` accepts, so it can be passed to a shell without quoting.
fn usable_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Wrap `script` so it runs under `lpi`, whose snapshots arrive on stderr.
/// The command's own output reaches stdout unchanged, both streams of it. A
/// key that a shell would read as syntax yields nothing, because the wrapper
/// is built as shell text.
pub fn wrap_script(script: &str, key: &str, db: Option<&Path>) -> Option<String> {
    if !usable_key(key) {
        return None;
    }
    let mut lpi = String::from("lpi pipe --learn-key ");
    lpi.push_str(key);
    lpi.push_str(" --json-stream");
    if let Some(dir) = db {
        let dir = dir.to_str()?;
        lpi.push_str(" --db ");
        lpi.push_str(dir);
    }
    Some(format!(
        "{script} 2>&1 | {lpi}\nexit \"${{PIPESTATUS[0]}}\"\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a second run against a learned model, so the parser is tested against what the program writes.
    const MATCHED: &str = r#"{"progress":0.6012861728668213,"units_done":2,"units_total":3,"units_pct":66.66666666666666,"has_times":true,"elapsed_seconds":0.00018,"elapsed_known":true,"ref_duration_seconds":0.000311,"eta_kind":"none","pace":0,"match_rate":1,"confidence":"high","current_lines":2,"matched_lines":2,"novel_lines":0,"overflow_lines":0}"#;

    /// Captured from a run that recorded a baseline: no model yet, so no ETA.
    const BASELINE: &str = r#"{"progress":0,"units_done":0,"units_total":0,"units_pct":0,"has_times":false,"elapsed_seconds":0,"elapsed_known":true,"ref_duration_seconds":0,"eta_kind":"none","pace":0,"match_rate":0,"confidence":"none","current_lines":1,"matched_lines":0,"novel_lines":1,"overflow_lines":0}"#;

    #[test]
    fn a_snapshot_line_reads_every_field() {
        let snapshot = parse_snapshot(MATCHED).expect("a snapshot line parses");
        assert_eq!(snapshot.units_done, 2);
        assert_eq!(snapshot.units_total, 3);
        assert_eq!(snapshot.confidence, "high");
        assert_eq!(snapshot.match_rate, 1.0);
        assert!((snapshot.progress - 0.601_286_172_866_821_3).abs() < f64::EPSILON);
        assert!(snapshot.eta_seconds.is_none(), "no ETA is absent, not zero");
    }

    #[test]
    fn a_baseline_snapshot_reads_without_an_eta() {
        let snapshot = parse_snapshot(BASELINE).expect("a snapshot line parses");
        assert_eq!(snapshot.progress, 0.0);
        assert_eq!(snapshot.confidence, "none");
        assert!(snapshot.eta_seconds.is_none());
    }

    #[test]
    fn the_streams_own_notices_are_not_snapshots() {
        for line in [
            "no model for key \"grokprobe\" yet -- recording baseline run",
            "learned run (3 lines, 0s) into key \"grokprobe\" (2 runs)",
            "Error: run not learned: model: need at least 2 nonempty log lines",
            "compiling a",
            "",
        ] {
            assert!(parse_snapshot(line).is_none(), "{line} is not a snapshot");
        }
    }

    #[test]
    fn the_wrapper_keeps_the_command_status() {
        let wrapped =
            wrap_script("cargo build", "grok-build", None).expect("a plain key is usable");
        assert!(wrapped.starts_with("cargo build 2>&1 | lpi pipe --learn-key grok-build"));
        assert!(wrapped.contains("--json-stream"));
        // A pipeline reports its last element, so the command's own status is restored rather than left to lpi's.
        assert!(wrapped.contains("exit \"${PIPESTATUS[0]}\""));
    }

    #[test]
    fn the_wrapper_names_the_model_directory_when_given_one() {
        let wrapped =
            wrap_script("make", "b", Some(Path::new("/tmp/lpidb"))).expect("a plain key is usable");
        assert!(wrapped.contains("--db /tmp/lpidb"));
    }

    #[test]
    fn a_key_a_shell_would_read_as_syntax_is_refused() {
        for key in ["", "a b", "a;rm -rf", "$(x)", "a|b", "a\nb"] {
            assert!(
                wrap_script("make", key, None).is_none(),
                "{key:?} must not reach a shell"
            );
        }
    }
}
