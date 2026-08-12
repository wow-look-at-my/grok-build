# AGENTS.md

Guidelines for autonomous coding agents working in this repository.

## Push-first workflow (most important)

CI is the source of truth and builds every pushed branch. A branch that is
only built locally is not production-real, and holding work back while you
verify serially wastes time when a parallel CI build could be running.

**PUSH FIRST, VERIFY AFTER.**

- As soon as a feature branch compiles (`cargo check` on the touched crate
  passes), commit it and push it so GitHub Actions gets a head start on the
  build.
- Do not let real-time verification of every edge case block the push. Push a
  compiling, self-consistent branch promptly, then continue verifying (unit
  tests, integration tests, evidence capture) while CI runs.
- Follow up on CI results and fix any failures reported for the pushed branch
  in follow-up commits rather than deferring the push.
- A feature branch must be created from freshly-pulled `master` and pushed
  with an explicit upstream: `git push --set-upstream origin <branch>`
  (or rely on `push.autoSetupRemote`).

## Branch hygiene

- Always branch off `master`, never off another WIP branch.
- Commit messages: concise, imperative mood, describing the change.
- Keep the working tree clean before switching context; use `git stash` /
  `git stash pop` for temporary changes and restore them promptly.

## Verification

- `cargo check -p <touched-crate>` before pushing.
- `cargo test -p <touched-crate>` for the crate you changed.
- Prefer committing real tests that drive the shipped code (not mocks of the
  unit under test, not hand-built expected objects).

## CI-status feature notes

- The GitHub CI-status dot lives in `crates/codegen/xai-grok-pager/src/ci_status.rs`
  (pure `gh` invocation + tri-state mapping + HSV-value animation) and is wired
  into the session status bar in `src/app/agent_view/render.rs`.
- The yellow "in progress" dot animates its HSV value in a sine wave between
  25% and 80% (see `ci_status::animate_value`).

## Self-hosted CI runner

- `vars.CI_RUNNER` selects the org's self-hosted runner; CI falls back to
  hosted `ubuntu-latest` wherever that variable is unset. The image is lean,
  so the workflow installs the C toolchain, rustup, ripgrep and protoc rather
  than assuming either environment.
- That runner has no PID 1 that reaps orphans, so
  `xai-grok-pager-pty-harness`'s process-tree and `privacy_banner_e2e` cases
  fail there (`PTY grandchild leaked after controller Drop`). Those suites are
  `#[ignore]`d and CI runs only the pager's own `pty_e2e_*` tests, as it does
  on `master`. Fixing that needs a change to the runner image (a `--init`-style
  reaper), which belongs to the fleet owner — not a skip in this repo's tests.
