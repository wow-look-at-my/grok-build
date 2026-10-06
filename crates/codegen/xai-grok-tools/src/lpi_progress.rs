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

/// The argument vector that reads one progress snapshot of a command's log.
///
/// The command is not wrapped: a shell pipeline runs its elements in
/// subshells. A wrapped command would leave the shell whose state the
/// executor replays. `analyze` reports on the log as it stands.
pub fn read_argv(key: &str, log: &Path, db: Option<&Path>) -> Vec<String> {
    let mut argv = vec![
        "lpi".to_string(),
        "analyze".to_string(),
        "--key".to_string(),
        key.to_string(),
        "--json".to_string(),
    ];
    if let Some(dir) = db {
        argv.push("--db".to_string());
        argv.push(dir.to_string_lossy().into_owned());
    }
    argv.push(log.to_string_lossy().into_owned());
    argv
}

/// The model key for a command run in a directory, so runs of the same shape in
/// the same project share a model.
pub fn key_for(cwd: &str, command: &str) -> String {
    use std::hash::{Hash, Hasher};

    let head: String = command
        .split_whitespace()
        .next()
        .unwrap_or("command")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cwd.hash(&mut hasher);
    head.hash(&mut hasher);
    format!("grok-{head}-{:x}", hasher.finish())
}

/// The argument vector that folds a finished run into its model.
pub fn learn_argv(key: &str, log: &Path) -> Vec<String> {
    vec![
        "lpi".to_string(),
        "learn".to_string(),
        "--key".to_string(),
        key.to_string(),
        log.to_string_lossy().into_owned(),
    ]
}

/// The one-line reading for a snapshot, for a caller to surface.
pub fn progress_line(snapshot: &Snapshot) -> String {
    let mut line = format!("progress {:.0}%", snapshot.progress * 100.0);
    if snapshot.units_total > 0 {
        line.push_str(&format!(
            " ({}/{} units)",
            snapshot.units_done, snapshot.units_total
        ));
    }
    if let Some(eta) = snapshot.eta_seconds {
        line.push_str(&format!(", eta ~{}s", eta.round() as u64));
    }
    if snapshot.confidence != "none" {
        line.push_str(&format!(", confidence {}", snapshot.confidence));
    }
    line
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
    fn a_snapshot_reads_as_a_progress_line() {
        let snapshot = parse_snapshot(MATCHED).expect("a snapshot line parses");
        assert_eq!(
            progress_line(&snapshot),
            "progress 60% (2/3 units), confidence high"
        );

        // A baseline run carries no units and no confidence yet, so the line says only what is known.
        let baseline = parse_snapshot(BASELINE).expect("a snapshot line parses");
        assert_eq!(progress_line(&baseline), "progress 0%");
    }

    #[test]
    fn the_reader_follows_the_log_without_touching_the_command() {
        let argv = read_argv("grok-build", Path::new("/logs/1.log"), None);
        assert_eq!(
            argv,
            vec![
                "lpi",
                "analyze",
                "--key",
                "grok-build",
                "--json",
                "/logs/1.log"
            ]
        );
        // Nothing here reaches a shell, so a key is data rather than syntax.
        assert!(
            !argv
                .iter()
                .any(|arg| arg.contains("|") || arg.contains("2>&1"))
        );
    }

    #[test]
    fn the_reader_names_the_model_directory_when_given_one() {
        let argv = read_argv("b", Path::new("/logs/1.log"), Some(Path::new("/tmp/lpidb")));
        assert_eq!(
            argv,
            vec![
                "lpi",
                "analyze",
                "--key",
                "b",
                "--json",
                "--db",
                "/tmp/lpidb",
                "/logs/1.log"
            ]
        );
    }

    #[test]
    fn a_key_separates_projects_and_command_shapes() {
        let make_a = key_for("/proj/a", "make -j8");
        // Flags do not make a different model; the project and the program do.
        assert_eq!(make_a, key_for("/proj/a", "make"));
        assert_ne!(make_a, key_for("/proj/b", "make"));
        assert_ne!(make_a, key_for("/proj/a", "cargo build"));
        assert!(make_a.starts_with("grok-make-"), "{make_a}");
    }

    #[test]
    fn the_learner_names_the_key_and_the_log() {
        assert_eq!(
            learn_argv("k", Path::new("/logs/1.log")),
            vec!["lpi", "learn", "--key", "k", "/logs/1.log"]
        );
    }

    #[test]
    fn a_key_that_is_shell_syntax_stays_a_single_argument() {
        for key in ["", "a b", "a;rm -rf", "$(x)", "a|b", "a\nb"] {
            let argv = read_argv(key, Path::new("/logs/1.log"), None);
            assert_eq!(
                argv,
                vec!["lpi", "analyze", "--key", key, "--json", "/logs/1.log"],
                "{key:?} must stay one argument"
            );
        }
    }
}
