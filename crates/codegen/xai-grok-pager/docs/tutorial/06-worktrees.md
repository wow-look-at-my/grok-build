# Parallel Work: Worktrees

Want Grok working on a feature while you (or another Grok session) work on
something else in the same repo? **Git worktrees** give each session its own
isolated checkout — no stepping on each other's changes, no stashing.

## Start a session in a worktree

- **From anywhere:** press `Ctrl+N` (twice to confirm) for a new session,
  then choose the worktree option.
- **From the welcome screen:** press `Ctrl+W` (inside a git repo) to open
  the New Worktree dialog.
- **From the shell:**

  ```bash
  grok --worktree=my-feature "refactor the auth module"
  ```

  (Use `=` — otherwise the prompt is taken as the worktree name.)

## Where the checkouts live

Each worktree is a directory under your grok home: `~/.grok/worktrees/<repo>/<name>`, where `<repo>` is derived from the main checkout's path. The repository itself is untouched, so its git status and tracked `.gitignore` stay as they are.

Checkouts that a release wrote inside their own repository, at `.grok/worktrees/<name>`, keep working. They stay where they are, and nothing moves them.

## Why this is great

- Run two or three Grok sessions on the same repo simultaneously.
- Experiments stay isolated — if a change doesn't work out, your main
  checkout is untouched.
- When the work is done, apply the changes back like any git branch.

**`/fork`** copies your current conversation into a parallel session —
add a directive to point it at a task: `/fork try the async approach`.

Running several agents? The **dashboard** (`/dashboard` or `Ctrl+\`) shows
every session grouped by state — who needs input, who's working, who's done.

*Go deeper: `/docs Session Management`*
