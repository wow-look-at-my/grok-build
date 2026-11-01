//! Which argument lists the read-only `git` tool runs. The first argument
//! names the subcommand, and only an allowlisted one runs.

/// The subcommands that print a diff.
pub(super) const DIFF_FAMILY: &[&str] = &[
    "diff",
    "diff-files",
    "diff-index",
    "diff-tree",
    "log",
    "show",
    "whatchanged",
];

/// Subcommands with no mode that writes.
pub(super) const READ_ONLY: &[&str] = &[
    "annotate",
    "blame",
    "cat-file",
    "check-attr",
    "check-ignore",
    "check-mailmap",
    "check-ref-format",
    "cherry",
    "count-objects",
    "describe",
    "for-each-ref",
    "grep",
    "ls-files",
    "ls-tree",
    "merge-base",
    "name-rev",
    "range-diff",
    "rev-list",
    "rev-parse",
    "shortlog",
    "show-branch",
    "show-ref",
    "status",
    "version",
];

/// Subcommands that run only in a listing form.
pub(super) const RESTRICTED: &[&str] = &[
    "branch",
    "config",
    "notes",
    "reflog",
    "remote",
    "stash",
    "submodule",
    "symbolic-ref",
    "tag",
    "worktree",
];

/// `--output=<file>` writes the diff to a file. `--ext-diff` runs the configured external diff program.
const DIFF_DENIED: &[&str] = &["output", "ext-diff"];

/// The approved argument list, ready to follow `git` and its global options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Approved {
    pub args: Vec<String>,
}

/// Approve `args` (the arguments after `git`) or say why they cannot run.
pub(crate) fn approve(args: &[String]) -> Result<Approved, String> {
    let Some((verb, rest)) = args.split_first() else {
        return Err(
            "name a git subcommand, for example [\"log\", \"--oneline\", \"-n\", \"20\"]"
                .to_owned(),
        );
    };
    if verb.starts_with('-') {
        return Err(format!(
            "`{verb}`: the first argument must be the subcommand. Global options such as -C and -c are not accepted"
        ));
    }
    if let Some(arg) = rest.iter().find(|a| *a == "--help") {
        return Err(format!(
            "`{arg}` opens a manual page. Use -h for the usage text"
        ));
    }
    let verb = verb.as_str();
    if DIFF_FAMILY.contains(&verb) {
        deny_long(rest, DIFF_DENIED, verb)?;
        return Ok(with_no_ext_diff(verb, &[], rest));
    }
    if READ_ONLY.contains(&verb) {
        check_read_only(verb, rest)?;
        return Ok(plain(verb, rest));
    }
    if RESTRICTED.contains(&verb) {
        return restricted(verb, rest);
    }
    Err(format!(
        "`git {verb}` is not available in this read-only tool. It runs: {}",
        [DIFF_FAMILY, READ_ONLY, RESTRICTED].concat().join(", ")
    ))
}

fn plain(verb: &str, rest: &[String]) -> Approved {
    let mut args = vec![verb.to_owned()];
    args.extend(rest.iter().cloned());
    Approved { args }
}

/// `verb`, then `sub`, then `--no-ext-diff`, then the rest of the caller's arguments.
fn with_no_ext_diff(verb: &str, sub: &[&str], rest: &[String]) -> Approved {
    let mut args = vec![verb.to_owned()];
    args.extend(sub.iter().map(|s| (*s).to_owned()));
    args.push("--no-ext-diff".to_owned());
    args.extend(rest.iter().cloned());
    Approved { args }
}

fn check_read_only(verb: &str, rest: &[String]) -> Result<(), String> {
    match verb {
        "range-diff" => deny_long(rest, DIFF_DENIED, verb),
        // `-O` / `--open-files-in-pager` run a program on the matched files.
        "grep" => {
            deny_long(rest, &["open-files-in-pager"], verb)?;
            deny_short(rest, 'O', verb)
        }
        // `--filters` runs the clean and smudge filter programs.
        "cat-file" => deny_long(rest, &["filters"], verb),
        _ => Ok(()),
    }
}

/// Refuse a long option that is a denied option or a prefix of one.
fn deny_long(args: &[String], denied: &[&str], verb: &str) -> Result<(), String> {
    for arg in args {
        let Some(name) = long_name(arg) else { continue };
        if let Some(hit) = denied.iter().find(|d| d.starts_with(name)) {
            return Err(format!(
                "`{arg}` is refused: `git {verb} --{hit}` writes a file or runs a configured program"
            ));
        }
    }
    Ok(())
}

/// Refuse a short option anywhere in a short-option bundle such as `-nO`.
fn deny_short(args: &[String], denied: char, verb: &str) -> Result<(), String> {
    for arg in args {
        if short_bundle(arg).is_some_and(|b| b.contains(denied)) {
            return Err(format!(
                "`{arg}` is refused: `git {verb} -{denied}` runs a program. Pass a pattern with `-e <pattern>` as a separate argument"
            ));
        }
    }
    Ok(())
}

/// The option name of `--name` or `--name=value`, or `None` for anything else.
fn long_name(arg: &str) -> Option<&str> {
    let name = arg.strip_prefix("--")?;
    let name = name.split_once('=').map_or(name, |(n, _)| n);
    (!name.is_empty()).then_some(name)
}

/// The letters of a short-option bundle (`-av` gives `av`), or `None`.
fn short_bundle(arg: &str) -> Option<&str> {
    let bundle = arg.strip_prefix('-')?;
    (!bundle.is_empty() && !bundle.starts_with('-')).then_some(bundle)
}

/// The options a restricted subcommand accepts.
#[derive(Default)]
struct Spec {
    /// Long or short options that take no value, written in full.
    flags: &'static [&'static str],
    /// More options that take no value. `config` keeps its read actions here.
    actions: &'static [&'static str],
    /// Long options that take a value. Only `--opt=value` is accepted.
    valued: &'static [&'static str],
    /// Long options whose value is optional and never taken from the next argument.
    optional_value: &'static [&'static str],
    /// Short options that take no value and may be bundled, such as `-av`.
    shorts: &'static str,
    /// A short option that takes digits in the same argument only, such as `-n5`.
    short_number: Option<char>,
}

/// Check every option against `spec` and return the positional arguments.
fn parse<'a>(verb: &str, args: &'a [String], spec: &Spec) -> Result<Vec<&'a str>, String> {
    let mut positionals = Vec::new();
    let mut options_done = false;
    for arg in args {
        if options_done || arg == "-" || !arg.starts_with('-') {
            positionals.push(arg.as_str());
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, value) = match rest.split_once('=') {
                Some((n, _)) => (n, true),
                None => (rest, false),
            };
            let full = format!("--{name}");
            let ok = if value {
                spec.valued.contains(&full.as_str()) || spec.optional_value.contains(&full.as_str())
            } else {
                spec.flags.contains(&full.as_str())
                    || spec.actions.contains(&full.as_str())
                    || spec.optional_value.contains(&full.as_str())
            };
            if ok {
                continue;
            }
            if !value && spec.valued.contains(&full.as_str()) {
                return Err(format!(
                    "`{arg}` needs its value in the same argument: write `{arg}=<value>`. A separate value is refused because git would read the next argument as the value"
                ));
            }
            return Err(format!(
                "`git {verb} {arg}` is not accepted by this read-only tool"
            ));
        }
        if spec.flags.contains(&arg.as_str()) {
            continue;
        }
        let mut chars = arg.chars();
        chars.next();
        while let Some(c) = chars.next() {
            if spec.short_number == Some(c) {
                if chars.as_str().bytes().all(|b| b.is_ascii_digit()) {
                    break;
                }
                return Err(format!(
                    "`{arg}`: -{c} takes only digits in the same argument"
                ));
            }
            if !spec.shorts.contains(c) {
                return Err(format!(
                    "`git {verb} -{c}` (in `{arg}`) is not accepted by this read-only tool"
                ));
            }
        }
    }
    Ok(positionals)
}

fn restricted(verb: &str, rest: &[String]) -> Result<Approved, String> {
    match verb {
        "branch" => branch(rest),
        "tag" => tag(rest),
        "remote" => remote(rest),
        "config" => config(rest),
        "stash" => stash(rest),
        "worktree" => worktree(rest),
        "notes" => notes(rest),
        "reflog" => reflog(rest),
        "symbolic-ref" => symbolic_ref(rest),
        "submodule" => submodule(rest),
        _ => unreachable!("`{verb}` is in RESTRICTED without a rule"),
    }
}

/// Options that filter or format a branch or tag listing.
const LISTING_VALUED: &[&str] = &[
    "--contains",
    "--format",
    "--merged",
    "--no-contains",
    "--no-merged",
    "--points-at",
    "--sort",
];

/// `git branch` lists. A positional pattern needs `--list`, because without
/// it the positional names a branch to create.
fn branch(rest: &[String]) -> Result<Approved, String> {
    let spec = Spec {
        flags: &[
            "--all",
            "--ignore-case",
            "--list",
            "--no-abbrev",
            "--no-color",
            "--no-column",
            "--omit-empty",
            "--quiet",
            "--remotes",
            "--show-current",
            "--verbose",
        ],
        valued: LISTING_VALUED,
        optional_value: &["--abbrev", "--color", "--column"],
        shorts: "aiqrv",
        ..Spec::default()
    };
    let positionals = parse("branch", rest, &spec)?;
    if !positionals.is_empty() && !rest.iter().any(|a| a == "--list") {
        return Err(format!(
            "`git branch {}` creates a branch. To filter the listing, add `--list` before the patterns",
            positionals.join(" ")
        ));
    }
    Ok(plain("branch", rest))
}

/// `git tag` lists. A positional pattern needs `-l` or `--list`, because
/// without it the positional names a tag to create.
fn tag(rest: &[String]) -> Result<Approved, String> {
    let spec = Spec {
        flags: &[
            "--ignore-case",
            "--list",
            "--no-color",
            "--no-column",
            "--omit-empty",
        ],
        valued: LISTING_VALUED,
        optional_value: &["--color", "--column"],
        shorts: "il",
        short_number: Some('n'),
        ..Spec::default()
    };
    let positionals = parse("tag", rest, &spec)?;
    let listing = rest
        .iter()
        .any(|a| a == "--list" || short_bundle(a).is_some_and(|b| b.contains('l')));
    if !positionals.is_empty() && !listing {
        return Err(format!(
            "`git tag {}` creates a tag. To filter the listing, add `--list` before the patterns",
            positionals.join(" ")
        ));
    }
    Ok(plain("tag", rest))
}

/// `git remote`, `git remote -v` and `git remote get-url <name>`.
fn remote(rest: &[String]) -> Result<Approved, String> {
    match rest.split_first() {
		None => Ok(plain("remote", rest)),
		Some((first, more)) if (first == "-v" || first == "--verbose") && more.is_empty() => {
			Ok(plain("remote", rest))
		}
		Some((first, more)) if first == "get-url" => {
			let spec = Spec {
				flags: &["--all", "--push"],
				..Spec::default()
			};
			if parse("remote get-url", more, &spec)?.len() != 1 {
				return Err("`git remote get-url` takes exactly one remote name".to_owned());
			}
			Ok(plain("remote", rest))
		}
		Some(_) => Err(
			"only `git remote`, `git remote -v` and `git remote get-url <name>` run in this read-only tool"
				.to_owned(),
		),
	}
}

/// `git config get`, `git config list`, or a legacy read action such as
/// `--get`. The subcommand or the action flag must come first.
fn config(rest: &[String]) -> Result<Approved, String> {
    const READ_ACTIONS: &[&str] = &[
        "--get",
        "--get-all",
        "--get-color",
        "--get-colorbool",
        "--get-regexp",
        "--get-urlmatch",
        "--list",
    ];
    const MODIFIERS: &[&str] = &[
        "--all",
        "--bool",
        "--bool-or-int",
        "--expiry-date",
        "--fixed-value",
        "--global",
        "--includes",
        "--int",
        "--local",
        "--name-only",
        "--no-includes",
        "--no-type",
        "--null",
        "--path",
        "--regexp",
        "--show-origin",
        "--show-scope",
        "--system",
        "--worktree",
    ];
    const VALUED: &[&str] = &[
        "--blob",
        "--default",
        "--file",
        "--type",
        "--url",
        "--value",
    ];
    /// Subcommand names that write.
    const WRITERS: &[&str] = &["edit", "remove-section", "rename-section", "set", "unset"];

    let Some((first, more)) = rest.split_first() else {
        return Err(
            "name a read: `git config get <key>`, `git config list`, or `git config --get <key>`"
                .to_owned(),
        );
    };
    let positionals = if first == "get" || first == "list" {
        let spec = Spec {
            flags: MODIFIERS,
            valued: VALUED,
            shorts: "z",
            ..Spec::default()
        };
        parse("config", more, &spec)?
    } else if READ_ACTIONS.contains(&first.as_str()) || first == "-l" {
        let spec = Spec {
            flags: MODIFIERS,
            actions: READ_ACTIONS,
            valued: VALUED,
            shorts: "lz",
            ..Spec::default()
        };
        parse("config", more, &spec)?
    } else {
        return Err(format!(
            "`git config {first}` is not a read. Start with `get`, `list`, `--get`, `--get-all`, `--get-regexp` or `--list`"
        ));
    };
    if let Some(w) = positionals.iter().find(|p| WRITERS.contains(p)) {
        return Err(format!("`{w}` is a git config subcommand that writes"));
    }
    Ok(plain("config", rest))
}

/// `git stash list` and `git stash show`.
fn stash(rest: &[String]) -> Result<Approved, String> {
    match rest.split_first() {
        Some((sub, more)) if sub == "list" || sub == "show" => {
            deny_long(more, DIFF_DENIED, "stash")?;
            Ok(with_no_ext_diff("stash", &[sub.as_str()], more))
        }
        _ => {
            Err("only `git stash list` and `git stash show` run in this read-only tool".to_owned())
        }
    }
}

/// `git worktree list`.
fn worktree(rest: &[String]) -> Result<Approved, String> {
    match rest.split_first() {
        Some((sub, more)) if sub == "list" => {
            let spec = Spec {
                flags: &["--porcelain", "--verbose"],
                valued: &["--expire"],
                shorts: "vz",
                ..Spec::default()
            };
            if !parse("worktree list", more, &spec)?.is_empty() {
                return Err("`git worktree list` takes no positional arguments".to_owned());
            }
            Ok(plain("worktree", rest))
        }
        _ => Err("only `git worktree list` runs in this read-only tool".to_owned()),
    }
}

/// `git notes [--ref=<ref>] [list | show] [<object>]`.
fn notes(rest: &[String]) -> Result<Approved, String> {
    let mut args = rest.iter();
    let mut sub = None;
    for arg in args.by_ref() {
        if arg.starts_with("--ref=") {
            continue;
        }
        sub = Some(arg);
        break;
    }
    let objects: Vec<&String> = args.collect();
    match sub {
        None => {}
        Some(s) if s == "list" || s == "show" => {
            if objects.len() > 1 || objects.iter().any(|o| o.starts_with('-')) {
                return Err(format!(
                    "`git notes {s}` takes at most one object and no options"
                ));
            }
        }
        Some(s) => {
            return Err(format!(
                "`git notes {s}`: only `git notes list` and `git notes show` run in this read-only tool. Pass a ref as `--ref=<ref>`"
            ));
        }
    }
    Ok(plain("notes", rest))
}

/// `git reflog`, `git reflog show`, `git reflog list` and `git reflog exists`.
/// The subcommand must come first, so no write subcommand is found later.
fn reflog(rest: &[String]) -> Result<Approved, String> {
    match rest.split_first() {
		None => Ok(plain("reflog", rest)),
		Some((sub, more)) if sub == "show" => {
			deny_long(more, DIFF_DENIED, "reflog")?;
			Ok(with_no_ext_diff("reflog", &["show"], more))
		}
		Some((sub, more)) if sub == "list" && more.is_empty() => Ok(plain("reflog", rest)),
		Some((sub, [name])) if sub == "exists" && !name.starts_with('-') => Ok(plain("reflog", rest)),
		_ => Err(
			"only `git reflog`, `git reflog show [<options>] [<ref>]`, `git reflog list` and `git reflog exists <ref>` run in this read-only tool"
				.to_owned(),
		),
	}
}

/// `git symbolic-ref <name>` reads a symbolic ref. A second positional sets it.
fn symbolic_ref(rest: &[String]) -> Result<Approved, String> {
    let spec = Spec {
        flags: &["--no-recurse", "--quiet", "--recurse", "--short"],
        shorts: "q",
        ..Spec::default()
    };
    if parse("symbolic-ref", rest, &spec)?.len() != 1 {
        return Err(
            "`git symbolic-ref` takes exactly one ref name. Two names set the ref".to_owned(),
        );
    }
    Ok(plain("symbolic-ref", rest))
}

/// `git submodule status`. Without the `status` word, a positional would be
/// read as the submodule command, so positionals need `status` first.
fn submodule(rest: &[String]) -> Result<Approved, String> {
    let spec = Spec {
        flags: &["--cached", "--quiet", "--recursive"],
        shorts: "q",
        ..Spec::default()
    };
    let (explicit, args) = match rest.split_first() {
        Some((sub, more)) if sub == "status" => (true, more),
        _ => (false, rest),
    };
    let positionals = parse("submodule", args, &spec)?;
    if !explicit && !positionals.is_empty() {
        return Err(
            "only `git submodule status [<options>] [<path>...]` runs in this read-only tool"
                .to_owned(),
        );
    }
    Ok(plain("submodule", rest))
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
