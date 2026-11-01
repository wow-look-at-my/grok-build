// A value option can take `--` as its value, so nothing after `--` is exempt.

use super::*;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

fn approved(line: &str) -> Vec<String> {
    match approve(&argv(line)) {
        Ok(a) => a.args,
        Err(e) => panic!("`git {line}` must run, got: {e}"),
    }
}

fn refused(line: &str) -> String {
    match approve(&argv(line)) {
        Ok(a) => panic!("`git {line}` must be refused, got {:?}", a.args),
        Err(e) => e,
    }
}

#[test]
fn reads_run() {
    for line in [
        "log --oneline -n 20",
        "log -p --stat -- src/main.rs",
        "log --output-indicator-new=+ -p",
        "show HEAD~1",
        "show --stat --format=%H%n%s HEAD",
        "diff",
        "diff --cached --name-only",
        "diff --no-index a b",
        "diff-tree -r HEAD",
        "blame -L 1,20 README.md",
        "status --porcelain=v1",
        "status -sb",
        "grep -n -e foo -- src",
        "grep -nI foo",
        "ls-files -z",
        "ls-tree -r HEAD",
        "cat-file -p HEAD",
        "cat-file --textconv HEAD:README.md",
        "rev-parse --show-toplevel",
        "rev-list --count HEAD",
        "rev-list --filter=blob:none HEAD",
        "describe --tags --dirty",
        "merge-base HEAD origin/master",
        "for-each-ref --format=%(refname) refs/heads",
        "shortlog -sn HEAD",
        "range-diff HEAD~3..HEAD~1 HEAD~2..HEAD",
        "version",
    ] {
        approved(line);
    }
}

#[test]
fn the_diff_family_gets_no_ext_diff_after_the_subcommand() {
    assert_eq!(approved("log -p HEAD"), argv("log --no-ext-diff -p HEAD"));
    assert_eq!(approved("diff"), argv("diff --no-ext-diff"));
    assert_eq!(
        approved("stash show -p stash@{0}"),
        argv("stash show --no-ext-diff -p stash@{0}")
    );
    assert_eq!(
        approved("reflog show -p"),
        argv("reflog show --no-ext-diff -p")
    );
    assert_eq!(approved("status"), argv("status"), "status prints no diff");
}

#[test]
fn verbs_that_write_are_refused() {
    for line in [
        "commit -m x",
        "add .",
        "rm a",
        "mv a b",
        "checkout main",
        "switch main",
        "restore a",
        "reset --hard",
        "merge x",
        "rebase main",
        "cherry-pick abc",
        "revert abc",
        "push",
        "pull",
        "fetch",
        "clone x",
        "init",
        "gc",
        "prune",
        "clean -fdx",
        "apply x.patch",
        "am x.patch",
        "format-patch -1",
        "update-ref refs/heads/x HEAD",
        "update-index --refresh",
        "hash-object -w a",
        "mktree",
        "merge-tree a b",
        "fsck --lost-found",
        "filter-branch",
        "bisect start",
        "sparse-checkout set a",
        "maintenance run",
        "repack -a",
        "ls-remote origin",
        "help log",
        "difftool",
        "instaweb",
    ] {
        refused(line);
    }
}

#[test]
fn global_options_and_help_are_refused() {
    refused("-c core.pager=sh log");
    refused("-C /tmp log");
    refused("--git-dir=/tmp/x log");
    refused("--exec-path=/tmp log");
    refused("log --help");
    refused("");
}

#[test]
fn flags_that_write_or_run_programs_are_refused_with_their_abbreviations() {
    for line in [
        "log --output=/tmp/x -p",
        "log --outp=/tmp/x -p",
        "log -p --out /tmp/x",
        "diff --output=f",
        "show --ext-diff",
        "diff --ext",
        "range-diff --output=f a b",
        "grep -O foo",
        "grep -nOvim foo",
        "grep --open-files-in-pager=vim foo",
        "grep --open foo",
        "cat-file --filters HEAD:a",
        "cat-file --filt HEAD:a",
        "stash show --output=f",
        "reflog show --output=f",
    ] {
        refused(line);
    }
}

/// A value option can take `--` as its value, so nothing after `--` is exempt.
#[test]
fn a_denied_flag_after_double_dash_is_still_refused() {
    refused("log --grep -- --output=/tmp/x");
    refused("diff -- --output=x");
}

#[test]
fn branch_runs_only_its_listing_forms() {
    for line in [
        "branch",
        "branch -a",
        "branch -avv",
        "branch --show-current",
        "branch --list feature/*",
        "branch --sort=-committerdate --format=%(refname:short)",
        "branch --contains=HEAD",
        "branch --merged=main -r",
        "branch --color=never --column",
    ] {
        approved(line);
    }
    for line in [
        "branch new",
        "branch -d old",
        "branch -D old",
        "branch -m a b",
        "branch -c a b",
        "branch -f main HEAD",
        "branch --set-upstream-to=origin/x",
        "branch -u origin/x",
        "branch --unset-upstream",
        "branch --edit-description",
        "branch --track x origin/x",
        "branch --create-reflog x",
        "branch --delete x",
        "branch --move a b",
        "branch --del x",
        "branch -l new",
        // `--sort` would take `--list` as its value and create `new`.
        "branch --sort --list new",
        "branch --contains --list new",
        "branch --format --list new",
        "branch --points-at -- new",
    ] {
        refused(line);
    }
}

#[test]
fn tag_runs_only_its_listing_forms() {
    for line in [
        "tag",
        "tag -l",
        "tag -l v1.*",
        "tag --list v1.*",
        "tag -n5 -l",
        "tag -n",
        "tag --sort=-v:refname",
        "tag --contains=HEAD",
        "tag --points-at=HEAD",
    ] {
        approved(line);
    }
    for line in [
        "tag v2",
        "tag -a v2 -m msg",
        "tag -s v2",
        "tag -d v1",
        "tag -f v1 HEAD",
        "tag -v v1",
        "tag --delete v1",
        "tag -n 5 v2",
        "tag --sort --list v2",
        "tag --contains -l v2",
        "tag -nx",
    ] {
        refused(line);
    }
}

#[test]
fn config_runs_only_reads() {
    for line in [
        "config --get user.name",
        "config --get-all remote.origin.fetch",
        "config --get-regexp ^remote\\.",
        "config --list",
        "config -l --show-origin",
        "config --list --file=.gitmodules",
        "config get user.name",
        "config get --all --show-scope remote.origin.fetch",
        "config list --global",
    ] {
        approved(line);
    }
    for line in [
        "config user.name x",
        "config user.name",
        "config set user.name x",
        "config unset user.name",
        "config edit",
        "config -e",
        "config --edit",
        "config --add a.b c",
        "config --unset a.b",
        "config --unset-all a.b",
        "config --replace-all a.b c",
        "config --rename-section a b",
        "config --remove-section a",
        "config --global user.name x",
        // `--file` would take `--get` as its value, and both names would set a key.
        "config --file --get a.b c",
        "config -f x --get a.b",
        "config --get set a.b c",
        "config --list unset a.b",
        "config get --add a.b",
    ] {
        refused(line);
    }
}

#[test]
fn remote_stash_worktree_notes_reflog_symbolic_ref_and_submodule_list_only() {
    for line in [
        "remote",
        "remote -v",
        "remote get-url origin",
        "remote get-url --push origin",
        "stash list",
        "stash list --date=relative",
        "stash show -p",
        "worktree list",
        "worktree list --porcelain",
        "notes",
        "notes list",
        "notes show HEAD",
        "notes --ref=review list",
        "reflog",
        "reflog show HEAD",
        "reflog show --date=iso -n 5",
        "reflog list",
        "reflog exists refs/heads/main",
        "symbolic-ref HEAD",
        "symbolic-ref --short -q HEAD",
        "submodule",
        "submodule --cached",
        "submodule status --recursive lib",
    ] {
        approved(line);
    }
    for line in [
        "remote add x url",
        "remote remove origin",
        "remote rename a b",
        "remote set-url origin x",
        "remote prune origin",
        "remote update",
        "remote show origin",
        "remote -v add x y",
        "stash",
        "stash push",
        "stash pop",
        "stash drop",
        "stash clear",
        "stash apply",
        "worktree add ../x",
        "worktree remove x",
        "worktree prune",
        "worktree list extra",
        "notes add -m x",
        "notes remove HEAD",
        "notes --ref review add",
        "notes show --force HEAD",
        "reflog expire --all",
        "reflog delete HEAD@{1}",
        "reflog drop",
        "reflog --all",
        "reflog HEAD",
        "reflog exists",
        "symbolic-ref HEAD refs/heads/x",
        "symbolic-ref -d HEAD",
        "symbolic-ref -m msg HEAD refs/heads/x",
        "symbolic-ref",
        "submodule update",
        "submodule --quiet update",
        "submodule init",
        "submodule add url",
        "submodule foreach rm -rf .",
        "submodule summary",
    ] {
        refused(line);
    }
}

#[test]
fn a_refusal_names_the_way_forward() {
    assert!(refused("branch new").contains("--list"));
    assert!(refused("branch --sort -committerdate").contains("--sort=<value>"));
    assert!(refused("commit -m x").contains("not available"));
}
