# File copy/move and git deletion safety

Rules Grok Build enforces so relocating or deleting code cannot silently destroy history.

These rules are also loaded into the agent system prompt. The tools refuse the banned actions; do not work around them.

## Relocating code is `cp` / `git mv`, never a rewrite

When merging or moving an existing file (for example folding `tml-test` into `tml`'s `cli` package), **do not** retype the file with `write` or empty-`old_string` `search_replace`.

Correct sequence:

1. `copy_file` / `move_file`, or `cp` / `git mv` in the shell
2. A **minimal** edit for package name, flags, or imports

`write` and `search_replace` descriptions forbid using those tools as a relocate mechanism. A write whose content is substantially identical to another repo file is rejected and names the similar file.

## Never `rm` a non-ignored git-repo file

Untracked `rm` of a non-ignored file in a git repository is forbidden, whether the file is tracked or not.

Required sequence:

1. Commit the file (if it is not already committed)
2. `git rm` in a **new** commit

Gitignored / scratch files (`target/`, `node_modules/`, `/tmp`, …) may still be `rm`'d.

## History-destroying workarounds are banned

Do not hide a deletion with:

- `git commit --amend` after `git rm` of a just-committed file
- `git reset --hard`
- `git filter-branch` / `git filter-repo`
- force-push of rewritten history

`filter-branch` is not a feature we implement; it is refused.

## Enforcement seams

| Seam | What it does |
| ---- | ------------ |
| bash PreToolUse (`crates/codegen/xai-grok-tools/src/implementations/grok_build/bash`) | Rejects `rm`/`unlink` of non-ignored git-repo files; rejects `git reset --hard`, `git filter-branch`/`filter-repo`, and `git commit --amend` that would drop a just-committed file |
| `session/git.rs` amend (`crates/codegen/xai-grok-workspace/src/session/git.rs`) | The workspace commit RPC refuses `--amend` when the index deletes files that HEAD just added |
| auto_mode git rules (`crates/codegen/xai-grok-workspace/src/permission/auto_mode`) | `git rm` is routine; `git commit --amend`, `git reset --hard`, filter-branch, and force-push are never auto-allowed |

Related: [Permissions and Safety](22-permissions-and-safety.md).
