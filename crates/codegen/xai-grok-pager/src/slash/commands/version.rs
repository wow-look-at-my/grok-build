//! `/version` -- which build this session is actually running.

use super::debug_context::{BinaryFreshness, BinaryIdentity, binary_identity};
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Report the version, the commit it was built from, and the binary on disk.
pub struct VersionCommand;

impl SlashCommand for VersionCommand {
    fn name(&self) -> &str {
        "version"
    }

    fn aliases(&self) -> &[&str] {
        &["about"]
    }

    fn description(&self) -> &str {
        "Show the running version, build commit, and binary"
    }

    fn usage(&self) -> &str {
        "/version"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Message(version_report(
            &xai_grok_version::installed(),
            xai_grok_version::BUILD_COMMIT_SHORT,
            xai_grok_version::BUILD_COMMIT,
            xai_grok_update::channel_label(),
            &binary_identity(),
        ))
    }
}

/// The reported lines. Split out from [`VersionCommand::run`] so the wording is
/// testable without a live process.
fn version_report(
    version: &str,
    commit_short: &str,
    commit_full: &str,
    channel_label: &str,
    identity: &BinaryIdentity,
) -> String {
    let mut out = format!("grok {version}{channel_label}");
    if commit_short == "unknown" {
        // A build from outside a git worktree; say so rather than printing a
        // word that reads like a commit.
        out.push_str(" (built outside a git worktree — no commit stamped)");
    } else {
        out.push_str(&format!(" (commit {commit_short})"));
        if let Some(url) = xai_grok_version::commit_github_url(commit_full) {
            out.push_str(&format!("\n{url}"));
        }
    }
    out.push_str(&format!(
        "\nrunning: {}",
        match &identity.running {
            Some(path) => path.display().to_string(),
            None => "unknown (current_exe() failed)".to_string(),
        }
    ));
    // The line that answers "am I on the build I just installed?", which is the
    // question that sends people here in the first place.
    match &identity.freshness {
        BinaryFreshness::Current => {
            out.push_str("\nup to date with the installed grok");
        }
        BinaryFreshness::Stale { installed } => out.push_str(&format!(
            "\nSTALE: {} is now {} — restart grok to run it",
            identity.installed_link.display(),
            installed.display()
        )),
        BinaryFreshness::Unmanaged => out.push_str(&format!(
            "\nunmanaged build (nothing installed at {})",
            identity.installed_link.display()
        )),
        BinaryFreshness::Unknown => {
            out.push_str("\ncannot compare with the installed grok: current_exe() failed");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn identity(freshness: BinaryFreshness) -> BinaryIdentity {
        BinaryIdentity {
            running: Some(PathBuf::from("/h/.grok/versions/0.2.7/grok")),
            installed_link: PathBuf::from("/h/.grok/bin/grok"),
            freshness,
        }
    }

    #[test]
    fn version_metadata() {
        let cmd = VersionCommand;
        assert_eq!(cmd.name(), "version");
        assert_eq!(cmd.aliases(), &["about"]);
        assert!(!cmd.takes_args());
    }

    /// The commit is the whole point of the command: it is what tells someone
    /// whether the fix they are waiting on is in the binary they are running.
    #[test]
    fn report_carries_version_commit_and_binary() {
        let report = version_report(
            "0.2.7",
            "324f371",
            "324f371aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            " [stable]",
            &identity(BinaryFreshness::Current),
        );
        assert!(
            report.starts_with("grok 0.2.7 [stable] (commit 324f371)"),
            "{report}"
        );
        assert!(
            report.contains(
                "https://github.com/wow-look-at-my/grok-build/commit/\
                 324f371aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            "{report}"
        );
        assert!(
            report.contains("running: /h/.grok/versions/0.2.7/grok"),
            "{report}"
        );
        assert!(report.contains("up to date"), "{report}");
    }

    #[test]
    fn a_stale_process_says_so_and_names_what_is_installed() {
        let report = version_report(
            "0.2.7",
            "324f371",
            "324f371aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "",
            &identity(BinaryFreshness::Stale {
                installed: PathBuf::from("/h/.grok/versions/0.2.8/grok"),
            }),
        );
        assert!(report.contains("STALE"), "{report}");
        assert!(report.contains("/h/.grok/versions/0.2.8/grok"), "{report}");
        assert!(report.contains("restart grok"), "{report}");
    }

    /// An unstamped build must not print "commit unknown" as if that were one.
    #[test]
    fn an_unstamped_build_says_no_commit_rather_than_unknown() {
        let report = version_report(
            "0.2.7",
            "unknown",
            "unknown",
            "",
            &identity(BinaryFreshness::Unmanaged),
        );
        assert!(report.contains("no commit stamped"), "{report}");
        assert!(!report.contains("commit unknown"), "{report}");
        assert!(!report.contains("/commit/"), "{report}");
    }
}
