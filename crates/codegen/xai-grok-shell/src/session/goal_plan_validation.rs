//! Validates the planner's `## Verification plan` against the authority
//! the OBJECTIVE grants. The plan is derived knowledge: it cannot hand
//! the implementer a permission the user never gave.

/// Header of the section this module governs.
const VERIFICATION_SECTION: &str = "verification plan";

/// `[artifact]` reads files, logs, hashes and build output on this
/// machine. `[live-system]` touches anything else.
const ARTIFACT_LABEL: &str = "[artifact]";
const LIVE_SYSTEM_LABEL: &str = "[live-system]";

/// Words that mean a step leaves this machine. The label is the
/// planner's claim; this list is the backstop that keeps a mislabelled
/// claim from carrying a device touch past the quote requirement.
const OFF_MACHINE_TOKENS: &[&str] = &[
    "ssh ",
    "scp ",
    "rsync ",
    "telnet",
    "rcon",
    "adb ",
    "fastboot",
    "kubectl",
    "ansible",
    "systemctl",
    "serial console",
    "remote host",
    "on the device",
    "on the phone",
    "on the vehicle",
    "production",
    "staging",
    "screenshot of the",
];

/// One thing wrong with the plan, phrased as the correction to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanViolation {
    /// 1-based index of the step within `## Verification plan`.
    pub(crate) step_index: usize,
    pub(crate) step: String,
    pub(crate) kind: PlanViolationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanViolationKind {
    /// Neither reach label is present.
    MissingReachLabel,
    /// Labelled `[live-system]` with no quote from the OBJECTIVE.
    LiveSystemWithoutQuote,
    /// Labelled `[artifact]` while naming an off-machine action.
    MislabelledOffMachine,
}

impl PlanViolation {
    /// The line the next planner attempt reads.
    pub(crate) fn correction(&self) -> String {
        let step = &self.step;
        let index = self.step_index;
        match self.kind {
            PlanViolationKind::MissingReachLabel => format!(
                "Step {index} carries no reach label. Prefix it with `{ARTIFACT_LABEL}` \
                 or `{LIVE_SYSTEM_LABEL}`: {step}"
            ),
            PlanViolationKind::LiveSystemWithoutQuote => format!(
                "Step {index} is `{LIVE_SYSTEM_LABEL}` but the OBJECTIVE does not ask for \
                 it. Either replace it with an `{ARTIFACT_LABEL}` check that proves the \
                 same claim, or quote the OBJECTIVE's own authorizing words in double \
                 quotes inside the step: {step}"
            ),
            PlanViolationKind::MislabelledOffMachine => format!(
                "Step {index} is labelled `{ARTIFACT_LABEL}` but acts outside this machine. \
                 Relabel it `{LIVE_SYSTEM_LABEL}` and quote the OBJECTIVE's authorizing \
                 words, or replace it with a check on files, logs, hashes or build \
                 output: {step}"
            ),
        }
    }
}

/// Collapse whitespace and case, so a quote survives the planner
/// rewrapping the user's sentence across lines.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_header(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

fn header_level(line: &str) -> usize {
    line.trim_start().chars().take_while(|c| *c == '#').count()
}

fn is_verification_header(line: &str) -> bool {
    is_header(line)
        && line
            .trim_start()
            .trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case(VERIFICATION_SECTION)
}

/// Strip a `1.` / `1)` / `- ` / `* ` / `+ ` marker. `None` for prose,
/// which is not a step and is not judged.
fn strip_step_marker(trimmed: &str) -> Option<&str> {
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return Some(rest.trim_start());
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &trimmed[digits..];
    rest.strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .map(str::trim_start)
}

/// Steps of the `## Verification plan` section, in order.
pub(crate) fn verification_steps(body: &str) -> Vec<String> {
    let mut level: Option<usize> = None;
    let mut steps = Vec::new();
    for line in body.lines() {
        if is_verification_header(line) {
            level = Some(header_level(line));
            continue;
        }
        let Some(section_level) = level else { continue };
        if is_header(line) && header_level(line) <= section_level {
            break;
        }
        if let Some(step) = strip_step_marker(line.trim_start())
            && !step.trim().is_empty()
        {
            steps.push(step.trim().to_string());
        }
    }
    steps
}

/// Every double-quoted span in `step`, straight and curly alike.
fn quoted_spans(step: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut open: Option<usize> = None;
    for (idx, ch) in step.char_indices() {
        if !matches!(ch, '"' | '\u{201c}' | '\u{201d}') {
            continue;
        }
        match open.take() {
            None => open = Some(idx + ch.len_utf8()),
            Some(start) => {
                let span = step[start..idx].trim();
                if !span.is_empty() {
                    spans.push(span.to_string());
                }
            }
        }
    }
    spans
}

/// True when a quoted span of `step` is the user's own words.
fn carries_objective_quote(step: &str, objective: &str) -> bool {
    let haystack = normalize(objective);
    if haystack.is_empty() {
        return false;
    }
    quoted_spans(step).into_iter().any(|span| {
        let needle = normalize(&span);
        // A one-word quote matches by accident against any objective
        // that happens to use the word. It authorizes nothing.
        needle.split(' ').count() >= 2 && haystack.contains(&needle)
    })
}

fn names_off_machine_action(step: &str) -> bool {
    let normalized = normalize(step);
    OFF_MACHINE_TOKENS
        .iter()
        .any(|token| normalized.contains(token))
}

/// Judge one step. `None` when it is admissible.
fn validate_step(index: usize, step: &str, objective: &str) -> Option<PlanViolation> {
    let lowered = step.to_lowercase();
    let live = lowered.contains(LIVE_SYSTEM_LABEL);
    let artifact = lowered.contains(ARTIFACT_LABEL);
    let violation = |kind| {
        Some(PlanViolation {
            step_index: index,
            step: step.to_string(),
            kind,
        })
    };

    if !live && !artifact {
        return violation(PlanViolationKind::MissingReachLabel);
    }
    if live {
        if carries_objective_quote(step, objective) {
            return None;
        }
        return violation(PlanViolationKind::LiveSystemWithoutQuote);
    }
    if names_off_machine_action(step) {
        return violation(PlanViolationKind::MislabelledOffMachine);
    }
    None
}

/// Every violation in `## Verification plan`, in step order. Empty means
/// the plan claims no authority the OBJECTIVE withheld. A plan with no
/// verification steps yields none: the contract's other rules govern it.
pub(crate) fn validate_plan(body: &str, objective: &str) -> Vec<PlanViolation> {
    verification_steps(body)
        .iter()
        .enumerate()
        .filter_map(|(idx, step)| validate_step(idx + 1, step, objective))
        .collect()
}

/// The violations rendered as the CONTEXT the next attempt reads.
pub(crate) fn rejection_feedback(violations: &[PlanViolation]) -> String {
    let mut out = String::from(
        "The previous plan was REJECTED. Every `## Verification plan` step must declare its \
         reach, and a step that touches anything outside this machine needs the user's own \
         words authorizing it. Rewrite the plan and fix each item below.\n",
    );
    for violation in violations {
        out.push_str("\n- ");
        out.push_str(&violation.correction());
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECTIVE: &str = "build an optimized test package and deploy it to my device";

    fn kinds(body: &str, objective: &str) -> Vec<PlanViolationKind> {
        validate_plan(body, objective)
            .into_iter()
            .map(|v| v.kind)
            .collect()
    }

    #[test]
    fn steps_are_read_only_from_the_verification_section() {
        let body = "# Plan\n\n## Acceptance criteria\n1. the package is optimized\n\n\
                    ## Verification plan\n1. [artifact] gating: read the build log\n\
                    2. [artifact] evidence: hash both binaries\n\n\
                    ## Task checklist\n- [ ] build it\n";
        assert_eq!(
            verification_steps(body),
            vec![
                "[artifact] gating: read the build log",
                "[artifact] evidence: hash both binaries",
            ],
        );
    }

    #[test]
    fn a_step_without_a_reach_label_is_rejected() {
        let body = "## Verification plan\n1. gating: read the build log\n";
        assert_eq!(kinds(body, OBJECTIVE), vec![
            PlanViolationKind::MissingReachLabel
        ]);
    }

    #[test]
    fn artifact_steps_pass() {
        let body = "## Verification plan\n\
                    1. [artifact] gating: compare sha256sum of the staged and deployed binary\n\
                    2. [artifact] evidence: read the deployed log's build-configuration line\n";
        assert!(validate_plan(body, OBJECTIVE).is_empty());
    }

    #[test]
    fn a_live_system_step_without_a_quote_is_rejected() {
        let body = "## Verification plan\n\
                    1. [live-system] gating: open the console and enable the stat overlay\n";
        assert_eq!(kinds(body, OBJECTIVE), vec![
            PlanViolationKind::LiveSystemWithoutQuote
        ]);
    }

    #[test]
    fn a_live_system_step_quoting_the_objective_is_admitted() {
        let objective = "deploy the test build and then restart the service for me";
        let body = "## Verification plan\n\
                    1. [live-system] gating: the user asked to \"restart the service\", so \
                    restart it and read the new pid\n";
        assert!(validate_plan(body, objective).is_empty());
    }

    #[test]
    fn a_quote_the_objective_never_contained_is_not_authorization() {
        let body = "## Verification plan\n\
                    1. [live-system] gating: \"enable the stat overlay\" on the running app\n";
        assert_eq!(kinds(body, OBJECTIVE), vec![
            PlanViolationKind::LiveSystemWithoutQuote
        ]);
    }

    #[test]
    fn a_single_word_quote_is_not_authorization() {
        let objective = "deploy the build to my device";
        let body = "## Verification plan\n1. [live-system] gating: \"deploy\" a stat dump\n";
        assert_eq!(kinds(body, objective), vec![
            PlanViolationKind::LiveSystemWithoutQuote
        ]);
    }

    #[test]
    fn a_quote_survives_rewrapping_and_case() {
        let objective = "Deploy the test build\nand restart the service afterwards";
        let body = "## Verification plan\n\
                    1. [live-system] gating: \"Restart   the  service\" and read the new pid\n";
        assert!(validate_plan(body, objective).is_empty());
    }

    #[test]
    fn curly_quotes_count_as_quotes() {
        let objective = "deploy the build and restart the service";
        let body = "## Verification plan\n\
                    1. [live-system] gating: \u{201c}restart the service\u{201d} then read the pid\n";
        assert!(validate_plan(body, objective).is_empty());
    }

    #[test]
    fn an_off_machine_step_labelled_artifact_is_rejected() {
        for step in [
            "[artifact] gating: ssh into the box and read the log",
            "[artifact] gating: adb shell the stat dump off the phone",
            "[artifact] gating: send an rcon command and read the reply",
            "[artifact] gating: capture a screenshot of the overlay",
            "[artifact] gating: curl the production endpoint",
            "[artifact] gating: read the staging service's health output",
        ] {
            let body = format!("## Verification plan\n1. {step}\n");
            assert_eq!(
                kinds(&body, OBJECTIVE),
                vec![PlanViolationKind::MislabelledOffMachine],
                "must reject: {step}",
            );
        }
    }

    #[test]
    fn an_off_machine_step_the_user_asked_for_is_admitted() {
        let objective = "deploy it, then ssh in and restart the daemon";
        let body = "## Verification plan\n\
                    1. [live-system] gating: the user said \"ssh in and restart the daemon\", \
                    so ssh in, restart it, and read the new pid\n";
        assert!(validate_plan(body, objective).is_empty());
    }

    #[test]
    fn bullets_and_paren_numbering_are_steps_too() {
        let body = "## Verification plan\n- gating: read the log\n2) gating: read the hash\n";
        assert_eq!(kinds(body, OBJECTIVE), vec![
            PlanViolationKind::MissingReachLabel,
            PlanViolationKind::MissingReachLabel,
        ]);
    }

    #[test]
    fn prose_inside_the_section_is_not_a_step() {
        let body = "## Verification plan\nAll checks run on the build machine.\n\
                    1. [artifact] gating: read the log\n";
        assert!(validate_plan(body, OBJECTIVE).is_empty());
    }

    #[test]
    fn a_plan_with_no_verification_section_yields_no_violations() {
        assert!(validate_plan("# Plan\n\n## Goal kind\nanalysis\n", OBJECTIVE).is_empty());
    }

    #[test]
    fn the_section_ends_at_the_next_same_level_header() {
        let body = "## Verification plan\n1. [artifact] gating: read the log\n\
                    \n## Non-goals\n- flip the overlay on the device\n";
        assert_eq!(verification_steps(body).len(), 1);
    }

    #[test]
    fn feedback_names_every_violation_and_its_step() {
        let body = "## Verification plan\n1. gating: read the log\n\
                    2. [live-system] gating: enable the overlay\n";
        let violations = validate_plan(body, OBJECTIVE);
        let feedback = rejection_feedback(&violations);
        assert!(feedback.contains("REJECTED"), "{feedback}");
        assert!(feedback.contains("Step 1"), "{feedback}");
        assert!(feedback.contains("Step 2"), "{feedback}");
        assert!(feedback.contains("enable the overlay"), "{feedback}");
    }
}
