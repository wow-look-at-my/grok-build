# CI-status feature notes

- The GitHub CI-status dot lives in `crates/codegen/xai-grok-pager/src/ci_status.rs` (pure `gh` invocation + tri-state mapping + HSV-value animation) and is wired into the session status bar in `src/app/agent_view/render.rs`.
- The yellow "in progress" dot animates its HSV value in a sine wave between 25% and 80% (see `ci_status::in_progress_dot_color`). The phase is WALL-CLOCK time (`pulse_elapsed`, one `CI_PULSE_PERIOD` per breath), not the frame tick. The loop's cadence moves with what the UI is doing (83 ms on an idle screen, ~30 fps while streaming) and a tick only advances on a frame that was drawn. A tick-counted pulse breathes faster the busier the screen is.
- A `--sandbox` session cannot spawn `gh` in the jail. The host worker (`xai-grok-sandbox/src/ci_host.rs`) answers fixed request shapes over the inherited fd: `gh-status` feeds the dot, `gh-pr` feeds the shell's `x.ai/pr/status` (`extensions/pr.rs`). `gh pr checks` puts its verdict in the exit code (1 failed, 8 pending) and prints the list either way, so both paths accept those codes.

- The dot is only realtime because things outside the render path keep it moving. Drop any one and it freezes at its last color, silently, on exactly the idle session that is watching CI:
  - the event loop's CI poll timer (`CI_POLL_INTERVAL`) keeps polling when no frame is being drawn — the render path refreshes only on frames it draws.
  - `set_change_notifier` gives the poller a way to ask for one repaint, and only when the color actually changed.
  - `ci_dot_animating` makes `tick_demand` report Slow while a run is in flight, which is what supplies the frames the pulse animates over.
