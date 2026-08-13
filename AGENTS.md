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

## Cost-indicator feature notes

- Per-message cost rides `XaiSessionUpdate::ResponseCompleted.cost_usd_ticks`,
  one per model call, and the pager attaches it to the message that call
  streamed (`AcpUpdateTracker::set_response_cost`). `TurnCompleted`'s
  prompt-scoped cost is the fallback for an agent that prices only whole turns;
  it stands down for any prompt a response already priced.
- The session total is the agent's own ledger
  (`ResponseCompleted`/`TurnCompleted.session_cost_usd_ticks`), not a sum over
  scrollback: rewound and never-rendered spend is real. The scrollback sum
  survives only as the fallback for an agent that reports no total.
- `ResponseCompleted` is the one buffered xAI update that is persisted — it is
  the only carrier of a message's cost, so a reload replays it and each message
  keeps its price. The indicator counts THIS run's spend: the agent's ledger is
  in-memory and restarts at reload, so a replayed total is not adopted and the
  scrollback sum stops being a valid fallback once anything priced is replayed
  (`AcpUpdateTracker::scrollback_sum_is_this_run`).
