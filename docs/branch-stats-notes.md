# Branch-stats notes

- The status bar shows `↑ahead ↓behind +ins -del` after the branch name (`branch_stats.rs`, drawn in `agent_view/render.rs`). A count of zero is not drawn.
- Ahead/behind is against the branch the checkout tracks. The order is. The current branch's local ref is never its own base, so a branch tracking `origin/<name>` compares against that remote copy.
- The +/- counts are the working tree against HEAD: staged, unstaged, and untracked files git does not ignore.
- The diff reads the working tree. It runs off-thread behind a 5 s throttle in its own cache, not on the render path.
- The +/- counts are a full work-tree scan, as costly as `git status`. A refresh reuses the last scan until the shared watcher's `ChangeCounter` moves or the HEAD tree changes (`cached_line_stats`). With no watcher, the scan runs at most once each `UNWATCHED_SCAN_INTERVAL`, and a warning names the cause. Never scan on each refresh: every idle pager then keeps several cores busy.
- Each refresh runs on its own short thread at the lowest CPU priority (`lower_thread_priority`): the highest nice value on Linux. This also covers which the scan's gix workers inherit, and the background class on macOS. Not the blocking pool: an unprivileged thread cannot raise its priority again. A pool thread will keep it for unrelated work.
