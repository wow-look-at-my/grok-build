# Background waits and the goal check

- No tool call waits on a background task. `get_task_output` returns a snapshot at once, and there is no `wait_tasks` tool. A `timeout_ms` key in a call is ignored. A model that wants to wait ends its turn. A running task's snapshot says so.
- A completion wakes the idle session. An active goal gets the same wakes: nothing drops a completion because a goal runs. A completed goal still drops late notifications (`maybe_drain_notifications`).
- The goal checker (`run_goal_round_end`) runs only when a round ends with no hold. `goal_check_hold` (`acp_session_impl/goal_hold.rs`) names the hold. It is a queued wake or pending notification, or live background work. Unfinished terminal tasks and monitors of this session, running subagents, and active workflow runs. On a hold the turn ends. No evaluator, no panel and no continuation directive run.
- `handle_turn_end` goes through `resume_goal_or_wait`. It queues the goal continuation only when there is no hold. The user sees one "Goal is waiting on …" line for each wait.
- `SessionCommand::BackgroundWorkSettled` covers a completion that sends no wake, such as a task the user killed or a wake dropped as already reported. The notification bridge sends it after each `TaskCompleted`. The subagent presenter sends it after each child. An idle session with an active goal and no hold then queues the continuation (`resume_goal_after_background_settled`).
- A task that a harness subagent (planner, verifier) left running is not the model's. `GoalBackground::harness_task_ids` holds its id. It holds no goal check, its wake is refused (`admit_task_completion_wake`), and its notification is dropped.
- A background task that never exits (a dev server, a monitor) holds the goal check while it runs. The goal discipline prompt tells the model to stop the tasks it does not need before it ends the turn.
- `pty_e2e_queue::goal_waits_on_background_work` checks the wait in the real pager with a scripted model, and CI runs it. `live_model` (`tests/it/live_model.rs` of the PTY harness) runs the same flow against a live model. No CI family selects it, because it needs a provider key. `GROK_LIVE_BASE_URL` is an OpenAI-compatible base URL, `GROK_LIVE_MODEL` is a model id of that provider, and `GROK_LIVE_KEY_ENV` names the variable that holds the key. `GROK_LIVE_GOAL_MODE=full` runs the planner and verifier flow in place of a lite goal:

```bash
cargo build -p xai-grok-pager-bin --bin xai-grok-pager --locked
PAGER_BINARY=$PWD/target/debug/xai-grok-pager \
GROK_LIVE_BASE_URL=https://router.huggingface.co/v1 \
GROK_LIVE_MODEL=moonshotai/Kimi-K3 \
GROK_LIVE_KEY_ENV=HF_TOKEN \
cargo test -p xai-grok-pager-pty-harness --test it --locked -- --ignored --exact \
	live_model::live_goal_turn_idles_on_background_work_and_is_checked_after_the_wake --nocapture
```
