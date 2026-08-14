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

- The dot is only realtime because three things outside the render path keep
  it moving; drop any one and it freezes at its last color, silently, on
  exactly the idle session that is watching CI:
  - the event loop's CI poll timer (`CI_POLL_INTERVAL`) keeps polling when no
    frame is being drawn — the render path refreshes only on frames it draws;
  - `set_change_notifier` gives the poller a way to ask for one repaint, and
    only when the color actually changed;
  - `ci_dot_animating` makes `tick_demand` report Slow while a run is in
    flight, which is what supplies the frames the pulse animates over.

## `/debug` feature notes

- `/debug <question>` injects the question plus an execution-context snapshot
  (`slash/commands/debug_context.rs`) through `CommandResult::InjectSkill`. Only
  `scroll`, `fps` and `log` are reserved; everything else is free text, so a
  question must never come back as an "unknown option" error again.
- Staleness is `current_exe()` versus a canonicalized `$GROK_HOME/bin/grok`.
  `current_exe()` resolves the symlink at exec time, so after an update the two
  disagree and the block says the running process is not what is on disk. Both
  sides must stay canonicalized or every symlinked install reads as stale.
- `GROK_*`/`XAI_*` values whose NAME looks like a credential are withheld —
  the prompt leaves the session and lands in the model's transcript.

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

## Why build-test is not on the self-hosted runner

Pointing `build-test` at `vars.CI_RUNNER` turns ~20 tests red, because they
assert on host semantics the org's lean image does not provide. Measured on
that runner, with unmodified test sources:

- no PID 1 that reaps orphans and no process-group signal delivery — every
  `*_grandchild*` case across `xai-grok-shell`, `xai-grok-test-support`,
  `xai-tty-utils` and the pager PTY harness (`PTY grandchild leaked after
  controller Drop`), plus `scope_teardown_kills_a_background_grandchild`,
  which hangs to the 60s timeout instead of failing;
- overlayfs reports `st_blocks=2` for every file, so `disk_usage_cmd` and
  `fs_size` measure ~1 KiB for anything;
- no UTF-8 locale by default, so `xai-grok-sandbox`'s
  `fails_closed_on_non_utf8_*` hit errno 84.

Every one of those is the test doing its job. Making them pass there means
weakening what they check, so the fix belongs to the runner image (an
init/reaper, a real filesystem for `/tmp`) and that image is the fleet's,
not this repo's. Revisit the runner once it has one; until then this job is
`runs-on: ubuntu-latest`, which is what `master` builds green on.
