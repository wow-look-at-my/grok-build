# AGENTS.md

Guidelines for autonomous coding agents working in this repository.

## Push-first workflow (most important)

CI is the source of truth and builds every pushed branch. A branch that is only built locally is not production-real, and holding work back while you verify serially wastes time. This applies when a parallel CI build can be running.

**PUSH FIRST, VERIFY AFTER.**

- As soon as a feature branch compiles (`cargo check` on the touched crate passes), commit it and push it so GitHub Actions gets a head start on the build.
- Do not let real-time verification of every edge case block the push. Push a compiling, self-consistent branch promptly, then continue verifying (unit tests, integration tests, evidence capture) while CI runs.
- Follow up on CI results and fix any failures reported for the pushed branch in follow-up commits rather than deferring the push.
- A feature branch must be created from freshly-pulled `master` and pushed with an explicit upstream: `git push --set-upstream origin <branch>` (or rely on `push.autoSetupRemote`).

## Branch hygiene

- Always branch off `master`, not off another WIP branch.
- Commit messages: concise, imperative mood, describing the change.
- Keep the working tree clean before switching context. Use `git stash` / `git stash pop` for temporary changes and restore them promptly.

## Verification

[docs/verification.md](docs/verification.md) holds this section.

## `--sandbox` jail notes

[docs/sandbox-jail-notes.md](docs/sandbox-jail-notes.md) holds this section.

## The darwin binary is compiled on Linux and linked on macOS

[docs/the-darwin-binary-is-compiled-on-linux-and-linked-on-macos.md](docs/the-darwin-binary-is-compiled-on-linux-and-linked-on-macos.md) holds this section.

## Release-number stamping and the shape of ci.yml

[docs/release-number-stamping-and-the-shape-of-ci-yml.md](docs/release-number-stamping-and-the-shape-of-ci-yml.md) holds this section.

## CI-status feature notes

[docs/ci-status-feature-notes.md](docs/ci-status-feature-notes.md) holds this section.

## Branch-stats notes

- The status bar shows `↑ahead ↓behind +ins -del` after the branch name (`branch_stats.rs`, drawn in `agent_view/render.rs`). A count of zero is not drawn.
- Ahead/behind is against the branch HEAD was created from. The order is: the reflog's `branch: Created from X`, an upstream that names a DIFFERENT branch, `origin/HEAD`, `origin/main`, `origin/master`, `main`, `master`. The current branch is never its own base, so `master` compares against `origin/master`.
- The +/- counts are the working tree against HEAD: staged, unstaged, and untracked files git does not ignore.
- The diff reads the working tree. It runs off-thread behind a 5 s throttle in its own cache, not on the render path.

## CI pipeline notes: the `gh` host worker, the `ci` tool, and the CI stop gate

[docs/ci-pipeline-notes-the-gh-host-worker-the-ci-tool-and-the-ci-stop-gate.md](docs/ci-pipeline-notes-the-gh-host-worker-the-ci-tool-and-the-ci-stop-gate.md) holds this section.

## A blocking wait is a gap, in Queue mode too

- The turn loop harvests queued follow-ups before each model request (`harvest_queued_prompts_into_interjections`). A turn parked in an interruptible wait tool (`get_task_output` with a wait, `wait_tasks`, `Await`) makes no request until the task ends. As a result, Queue mode held the row behind a task that can run for minutes.
- The shell now harvests at the wait too: when the wait starts (`tool_calls.rs`) and when a row arrives during it (`queue_input`). The harvested row aborts the wait, not the turn. The pager's parked-wait release (`release_queued_prompt_from`) no longer checks the mode either. Steer keeps its own promote path. An active goal and a pending send-now are exempt.

## Compaction report

- Every successful compaction writes `{session_dir}/compaction_reports/<checkpoint id>.md` (`helpers/compaction_report.rs`). It lists every item of the compacted history with its kind, bytes/4 estimate and a preview. It also holds the largest items, the full summary text, and the reseed arithmetic.
- "Tokens after" is a projection, not a measurement. `replace_conversation` scales the new history's estimate by `tokens_before ÷ estimate_at_last_response` and caps it at `tokens_before`. The report prints each factor. As a result, an inflated scale reads apart from a history that really stayed large.
- `AutoCompactCompleted` carries a one-line `breakdown` and the `report_path`. The pager draws both under "Context compacted".

## Compaction-failure reporting and mid-turn `/compact`

[docs/compaction-failure-reporting-and-mid-turn-compact.md](docs/compaction-failure-reporting-and-mid-turn-compact.md) holds this section.

## Harness model-slot notes

[docs/harness-model-slot-notes.md](docs/harness-model-slot-notes.md) holds this section.

## `/debug` feature notes

- `/debug <question>` injects the question plus an execution-context snapshot (`slash/commands/debug_context.rs`) through `CommandResult::InjectSkill`. Only `scroll`, `fps` and `log` are reserved. Everything else is free text. So a question must never come back as an "unknown option" error again.
- Staleness is `current_exe()` versus a canonicalized `$GROK_HOME/bin/grok`. `current_exe()` resolves the symlink at exec time, so after an update the two disagree. The block says the running process is not what is on disk. Both sides must stay canonicalized or every symlinked install reads as stale.
- `GROK_*`/`XAI_*` values whose NAME looks like a credential are withheld — the prompt leaves the session and lands in the model's transcript.
- `/debug` turns the firehose on. With no `GROK_DEBUG_LOG`/`GROK_LOG_FILE`, `install_firehose` installs the routing layer DORMANT behind `RuntimeGate`, and `debug_log::enable_firehose` wakes it. Spans pass the gate while it is closed. The routing layer must see a session span when it opens, or that session's later events go to the fallback file.
- The agent can be a separate leader process, so the pager's switch does not reach it. The `/debug` prompt block carries `ENABLE_FIREHOSE_META`, and the shell's `prompt` handler calls `enable_firehose` on it. Events before the switch are not in the log. The injected context says so.

## Shift+Tab mode ring notes

[docs/shift-tab-mode-ring-notes.md](docs/shift-tab-mode-ring-notes.md) holds this section.

## `/goal` role-model notes

[docs/goal-role-model-notes.md](docs/goal-role-model-notes.md) holds this section.

## Plan approval starts a goal

- Plan mode is the interactive goal planner. Approving the plan calls `setup_goal_from_approved_plan` (`acp_session_impl/plan_goal.rs`). That creates a goal and copies `plan.md` to the goal's plan and baseline. And the planner never runs.
- A mid-turn approval sends the goal-start reminder as a deferred followup after the `exit_plan_mode` result. A resume approval puts it at the front of the implement turn.
- An active goal is never replaced, and a subagent or a session without the goal harness gets no goal. The plan-mode reminder asks for the planner's sections. It names the contract only when `goal_contract` is true, since that is the only case where approval makes a goal.

## Goal-plan-to-todos notes

[docs/goal-plan-to-todos-notes.md](docs/goal-plan-to-todos-notes.md) holds this section.

## The run log is the goal verifier's runtime evidence

[docs/the-run-log-is-the-goal-verifier-s-runtime-evidence.md](docs/the-run-log-is-the-goal-verifier-s-runtime-evidence.md) holds this section.

## Verification does not widen the goal

- A `## Verification plan` step reads back what the goal built. It is not a permit. The implementer read "do X to confirm Y" as an instruction to do X. A planner-invented check then became an action on a system nobody put in scope. Every place that demands verification says so now. Those are the planner prompt's `## Verification plan` contract, `goal_rules.md`'s VERIFY AS YOU GO, `goal_plan_block.md`, and the per-turn continuation directive.
- The planner is told to prefer reading what the work already produced over operating anything. Files, logs, hashes, build output and source are what it reads. That is the whole mechanism. There is no label grammar and no validator. An earlier attempt added a reach DSL, a keyword list and a reject-and-retry loop to a planner prompt that is already long. That buys rigidity rather than scope discipline.

## Lite `/goal` mode

- `/goal --lite <objective>` (the flag may also be the last token) runs no planner and no skeptic panel. `GoalOrchestration::mode` holds the choice. A snapshot with no `mode` field reads as `Full`.
- The per-round evaluator (`goal_evaluator.rs`) is the whole check. Its `candidate_complete` ends the goal (`complete_lite_goal`). Any other verdict sends the model back. And the directive carries the evaluator's evidence as the reason. The evaluator prompt changes with the mode, because a lite verdict is final.
- Every planner entry and plan-path read goes through `goal_planner_on()`, which is false for a lite goal. That includes the load-time reconcile, which otherwise pauses an active goal that has no plan.
- On the legacy driver, a lite `update_goal(completed: true)` makes one evaluator call and is refused with `LiteCheckNotMet` unless the verdict is `candidate_complete`.

## `/todo` capture feature notes

[docs/todo-capture-feature-notes.md](docs/todo-capture-feature-notes.md) holds this section.

## Thinking-summary notes

- Every non-empty thinking block gets a summary, subagents included (`acp_session_impl/thinking_summary.rs`). Thinking is drawn collapsed. As a result, the summary is the only part of a short block anyone reads.
- It arrives after its model call ends, keyed by `stream_start_ms`. The root agent and the child-session handler both route it to `set_thinking_summary` on their own tracker.
- A summarized thought is `RunStep::Transparent` in a verb-group run. It keeps its row rather than fold to height 0.
- The summary is strictly async: nothing waits for it. The row redraws when the summary lands.
- Headless `streaming-json` emits a `thinking_summary` line. The Messages format has no field for it and drops it.

## Streaming tool-call notes

- A call's arguments reach the pager as the model writes them. The path is `SamplingEvent::ToolCallDelta` → `XaiSessionUpdate::ToolCallDeltaChunk` → `AcpUpdateTracker::handle_tool_call_delta`. The real `ToolCall` then adopts the row those deltas built. So a call keeps the position it held while it was typed.
- The row shows the CONTENT. It does not show a progress number alone. `OtherToolCallBlock::streaming_preview` holds the decoded tail of the arguments. The block draws it under the header in every display mode. `Collapsed` is included, because that is the mode a call is in while it streams. A preview drawn only when expanded left a write that read `◆ write` and nothing else until the file was whole.
- `StreamingArgsTail` (`acp/streaming_args.rs`) decodes the JSON escapes as the fragments land. A provider splits the arguments at arbitrary offsets. So an escape sequence can straddle a fragment boundary. The decoder carries that state across. A raw fragment puts `\n` on the screen and draws a file write as one line thousands of columns wide.
- The tail is bounded in both directions. It drops the older lines. One line stops growing at its character cap. A tab becomes spaces. So a large write costs a fixed row height however long it runs.
- The byte count beside the name is what the tail cannot say. A tail of a large body looks the same at any size.
- The preview truncates each line. It never wraps one. Every fragment redraws it. A wrapped line changes the block's height as the model types, which makes the whole transcript jump.

## The todo list cannot be discarded or overwritten

- A todo is the user's. Nothing can delete one: an item leaves the actionable set only by becoming `Completed` or `Cancelled`, both of which name it by id. Text is changed by sending that id with new content.
- Every `todo_write` is a merge, and an item the call omits survives with its status untouched. `merge: false` used to clear the list and keep only what the call resent. This is how a status update that forgot the flag erased the user's list.
- `merge` is still accepted on the wire and ignored. It is `#[schemars(skip)]` now that both values behave the same — advertising it will describe a choice the tool no longer offers.
- `TodoState` has no `clear` and no remove of any shape. The guarantee lives in the data structure so a later caller cannot reach around it.
- The list only grows, and every `todo_write` echoes all of it. So `summarize_todo_state` echoes a completed or cancelled item as its first line, cut at `FINISHED_ITEM_ECHO_CHARS`. The state keeps the full text. The post-compaction reminder already collapses finished items to counts.
- opencode's `todowrite` sends a whole list with no ids, so it merges by ITEM TEXT, not by position. Position is not identity. Keying on it let a reordered or shorter list write one row's text over another's, which loses work as surely as a delete.

## Cost-indicator feature notes

[docs/cost-indicator-feature-notes.md](docs/cost-indicator-feature-notes.md) holds this section.

## Output-budget notes

[docs/output-budget-notes.md](docs/output-budget-notes.md) holds this section.

## Model-pricing resolution notes

[docs/model-pricing-resolution-notes.md](docs/model-pricing-resolution-notes.md) holds this section.

## Stream-timing notes

- `itl_intervals_ms` truncates every gap to whole milliseconds. A stream above many chunks/s reads as a run of zeros and `itl_p50_ms` reports 0 for one that stutters. `InferenceLatencyStats.chunk_offsets_us` keeps each content chunk's arrival offset from `stream_start` in microseconds instead, off the `Instant`s all backend streams already record.
- `GROK_LOG_STREAM_TIMING=1` adds it to `shell.turn.inference_done` in `~/.grok/logs/unified.jsonl`. Opt-in: it is one number per chunk on a log that is otherwise one line per model call. The gate is read once per process (`inference_metrics::log_stream_timing`), so one run's entries agree.
- The offsets are client-side SSE-parse times, so transport jitter is in them. They are not a measurement of server decode.

## Thinking-replay notes

[docs/thinking-replay-notes.md](docs/thinking-replay-notes.md) holds this section.

## Tool-schema fallback notes

- Every tool schema goes out as the tool published it (`ToolSchemaForm::Native`). A top-level `oneOf`/`anyOf`/`allOf` is valid JSON Schema. Anthropic's Messages API still rejects it with a 400, and MCP servers publish that shape.
- The sampler answers that 400 with `RetryDecision::RetryWithToolSchemaFallback`: `ConversationRequest::degrade_tool_schemas` moves the request to `NoTopLevelCombinators` and sends it again. Every builder reads the form (`tool_parameters`), so the fallback covers every backend. A schema with no top-level combinator is identical in both forms.
- The fallback merges the branches into one `type: object` (`conversation/tool_schema.rs`). `allOf` keeps every required field. `anyOf`/`oneOf` keep only the fields that every branch requires, and the description lists each branch's required set. The tool still validates its input against its own schema.
- `ModelRejections::tool_schemas` remembers the model. Later requests start in the fallback form instead of paying the same 400 first. The fallback runs one time per request: a second rejection is the real error.
- A provider names a bad tool by index (`tools.16.custom.input_schema`). `name_tool_indices` adds the tool's name to that path in the error text for the Messages, Chat Completions and Ollama clients. Responses is not covered, because its wire `tools` array also holds hosted tools.

## Tool-call provider-field notes

- A tool call carries keys this client only relays: `extra_content` (Google's spelling) and `provider_specific_fields` (a translating gateway's). Gemini 3 rejects a replayed function call whose thought signature is missing, and that signature reaches an OpenAI-shaped client only inside one of them.
- `ToolCall::vendor` holds them from the response — including off the streaming chunk that opens the call, which is where Gemini puts the signature. `ToolCallRequest` flattens them back onto the replay. Nothing reads them: verbatim is the only form the provider accepts.
- The allowlist (`TOOL_CALL_VENDOR_KEYS`) is what keeps response-shaped bookkeeping out of the request. A provider that sends none leaves the map empty, and an empty map flattens to nothing, so its requests are unchanged.

## Goal-planner cancellation notes

- Nothing replans. A Send Now delivers its text to the planner already running (`SubagentEvent::Interject`, routed by the coordinator id the spawn publishes on the goal tracker) instead of cancelling it. An `Interrupted` reaching the loop is a bare cancel and is terminal. Retrying one spawned dead planners in 2.3 s before the attempt cap paused the goal.
- The planner runs off a slash command, not a turn. And a user Stop latches the session's Task spawns closed until a turn reopens them (`open_subagent_spawn_admission`). `maybe_run_goal_planner` reopens them itself. Without that, `/goal resume` after a Stop is rejected before a subagent exists, at latency 0, for every message the session has left.
- A pause the user asked for says so (`planner_cancelled_pause_message`). "Planning failed" on a cancel sends the reader hunting a broken planner that is doing exactly what it was told.

## Messages thinking-dialect notes

- Claude 4.6 replaced `thinking: {type:"adaptive"}` for `{type:"enabled", budget_tokens:N}`, and each generation rejects the other's spelling outright ("Input tag 'adaptive' ... does not match any of the expected tags"), so `build_messages_request` picks by model id (`speaks_adaptive_thinking`).
- The generation is parsed off the id itself (`claude_version`), because nothing else in the request carries it. Both spellings the family has used are read (`claude-haiku-4-5`, `claude-3-7-sonnet`), through a gateway prefix and a snapshot stamp. A name that is not a Claude is a gateway's own model and keeps the adaptive request it has always been sent.
- `output_config.effort` is 4.6-and-later too. So the older dialect sends the effort as `budget_tokens` instead and nothing beside it. `output_config.format` is untouched — structured outputs are not what 4.6 changed.
- A budget must clear the API's 1024 floor and stay under `max_tokens`. One that cannot do both leaves thinking off with a warning, rather than sending a request the API answers with a 400.

## CI compile-cache notes

[docs/ci-compile-cache-notes.md](docs/ci-compile-cache-notes.md) holds this section.

## The dependency tar: one cache entry per dependency set, and why it cannot grow

[docs/the-dependency-tar-one-cache-entry-per-dependency-set-and-why-it-cannot-grow.md](docs/the-dependency-tar-one-cache-entry-per-dependency-set-and-why-it-cannot-grow.md) holds this section.

## Why build-test is not on the self-hosted runner

Pointing `build-test` at `vars.CI_RUNNER` turns tests red, because they assert on host semantics the org's lean image does not provide. Measured on that runner, with unmodified test sources:

- no PID 1 that reaps orphans and no process-group signal delivery — every `*_grandchild*` case across `xai-grok-shell`, `xai-grok-test-support`, `xai-tty-utils` and the pager PTY harness (`PTY grandchild leaked after controller Drop`). Plus `scope_teardown_kills_a_background_grandchild`, which hangs to the 60s timeout instead of failing.
- overlayfs reports `st_blocks=2` for every file, so `disk_usage_cmd` and `fs_size` measure ~1 KiB for anything.
- no UTF-8 locale by default, so `xai-grok-sandbox`'s `fails_closed_on_non_utf8_*` hit errno 84.

Every one of those is the test doing its job. Making them pass there means weakening what they check, so the fix belongs to the runner image (an init/reaper, a real filesystem for `/tmp`). That image is the fleet's, not this repo's. Revisit the runner once it has one. Until then this job is `runs-on: ubuntu-22.04`, like every other Linux job in the workflow, which is what `master` builds green on.

## Todo-stop-gate notes

- The built-in todo gate is a participant in the turn-end STOP-HOOK gate, not a mechanism beside it (`acp_session_impl/turn.rs`, on `StopGateDecision::AllowStop`). It fires only after the user hooks allowed the stop. Its reminder rides the same `stop_hook_feedback` user message a hook block uses. It consumes the SAME `stop_continuations_this_turn` budget. So `MAX_STOP_HOOK_CONTINUATIONS_PER_TURN` is the stuck-release: a model that never engages its todos stops anyway.
- switches, and they are ORed, not ANDed (`todo_stop_gate_enabled`). The persisted `[ui].stop_gate_unfinished_todos` toggle ships ON and is the switch. `todo_gate.enabled` (remote `todo_gate_enabled`, or the `--todo-gate` CLI force-enable) is an opt-in on top, for a session whose toggle the user turned off. ANDing them is what shipped the feature dead: `TodoGateConfig::default().enabled` is false, so every default session took the `None` arm and the gate never ran.
- `todo_gate_applicable` is the other half and still binds. It allows no gate while the goal loop is active, because the continuation directive drives the loop there. It allows no gate for a prompt that carries no `<task_completion_discipline>` block.
- `todo_stop_gate_blocks` is pure and table-tested. The actor supplies the toggle, the shared continuation counter, and `evaluate_todo_gate` over the live todo state.

## `send_message` notes

[docs/send-message-notes.md](docs/send-message-notes.md) holds this section.

## History-flattening notes

[docs/history-flattening-notes.md](docs/history-flattening-notes.md) holds this section.

## Project-instruction `@import` notes

- An `@ref` in a discovered instruction file names a file to deliver (`prompt/agents_md_imports.rs`). Before it existed, this repo's `CLAUDE.md` shipped the literal line `@AGENTS.md` and none of the rules under it.
- An imported file is its OWN `AgentConfigFile`, placed right after the file that named it, rather than text spliced into the importer. That keeps the `## From:` path on every instruction. It also lets discovery's canonical-path dedup cover imports. A ref to a file discovery already found therefore adds nothing.
- The gitignore filter is discovery's, not the import path's. A ref is a deliberate instruction to read that file. Applying the filter to it makes a personal `CLAUDE.local.md` unimportable, which is the one thing people gitignore it for.
- A rule file's frontmatter is stripped from the RULE, not from what the rule imports. The import is read as written.
- `MAX_IMPORT_DEPTH` plus the seen-set bound the walk. The seen-set is what terminates a cycle. The depth cap only bounds a chain.

## Retry-visibility notes

- A retry says what failed and how long the wait is. `SamplingEvent::Retrying` already carried `reason`, which is the error's own `Display`. It now also carries `retry_in_ms`, the backoff the actor is about to sleep.
- Both ride `RetryState::Retrying` to the pager. It renders `Retrying in 27s (1/5): <reason>…` and drops the countdown once the wait is over. The retried request is then in flight. See `retry_label` in `views/turn_status.rs`.
- `retry_in_ms` is `None` for a retry that sleeps nothing. An image strip, a reasoning strip and a message-property strip are those. A shell older than the field also sends `None`.
- The countdown needs no tick source of its own. `tick_demand` already reports Fast for a session whose turn is running, which a retry always is.
- A server `Retry-After` is clamped to `MAX_RETRY_BACKOFF` on the 429 path too, not just the generic one. A per-minute bucket answers `Retry-After: 60`, and one attempt then sat idle for the whole minute. The limit had often cleared sooner.
- `RATE_LIMIT_RETRY_THRESHOLD` covers the wait the server asked for across several attempts. So the turn fails no earlier than before, and an attempt in between can find the limit clear.

## Stream-interruption retry notes

- A response stream that dies mid-body has its own retry budget: `STREAM_INTERRUPT_MAX_RETRIES` = 10, on the transport path's exponential backoff (2s, 4s, 8s, ... capped at `MAX_RETRY_BACKOFF`, jittered). `SamplingError::is_stream_interrupted` names the class. `request_task` charges it to `stream_retry_count` and not to the transport budget. A dropped connection is not a server fault, and the next 5xx still needs its own retries.
- The budget is a floor as well as a cap. A model configured with `max_retries = 3` still gets these 10. Only `max_retries = 0` (observe-only) and a caller that cannot take duplicate output (`retry_only_before_output` after output) get zero, through `stream_interrupt_budget`.
- A reqwest decode failure is in that class, and `is_retryable_reqwest` answers true for it. Its Display is "error decoding response body". A body that stopped arriving mid-read is transient, so calling it fatal ends a turn on one network blip. The same failure reaches the user as `EventStreamError` when the SSE stream is what broke.

## Output-rate floor notes

[docs/output-rate-floor-notes.md](docs/output-rate-floor-notes.md) holds this section.

## `/fork` and running subagents

- A fork copies `updates.jsonl`, and the child's load replays it. A `subagent_spawned` with no matching `subagent_finished` is therefore inherited. `prepare_replay_lines` reports it as unfinished and the child opens with that agent's row. The run itself stays the parent's, because `emit_subagent_notification` addresses the `parent_session_id` recorded at spawn. So the finish never reaches the child.
- The copy drops the records of every subagent that is still RUNNING at the fork point (`CopySessionOptions::carry_running_subagents`, default off). A subagent that already finished is history the conversation refers to. Its spawn and finish pair is copied either way. This is the same boundary the copy draws for workflow and goal projections.
- Running is decided over the lines the copy KEEPS. It is not decided over the whole source file. A `target_prompt_index` that cuts a finish away leaves a spawn the child reads as live. That spawn is dropped too.
- `/fork --agents` opts back in. The records are copied. The child's load then reconciles them the way a resumed session's are (`reconcile_orphaned_subagents_with_backend`). An agent this process still has running keeps its row. One that is gone is finished as cancelled. The child still cannot receive that run's output, because the run answers to the parent.
- The flag rides `ForkSessionRequest::include_agents` on `x.ai/session/fork` and `ResumeSessionInWorktreeRequest::include_agents` on the worktree fork. So `/fork --worktree --agents` behaves the same.

## Reasoning-effort support gate notes

[docs/reasoning-effort-support-gate-notes.md](docs/reasoning-effort-support-gate-notes.md) holds this section.

## Model-request parallelism cap

- `[ui].max_parallel_requests` (default 7, `0` = no cap) is the most model requests one process sends at once (`xai-grok-sampler/src/request_slots.rs`). A request past it waits in a FIFO queue. Every config load applies it (`apply_max_parallel_requests` in `util/config/campaigns.rs`), so a reload changes it live. A lowered cap never cuts off a request in flight.
- The slot is taken in the client's send methods and held until the response stream is dropped. So every caller is covered: the sampler actor, `conversation_collect`, and compaction's direct stream calls.
- The actor takes its slot in `run_one_attempt` BEFORE `FirstTokenDeadline::start`, and runs the send under `with_slot_held` so the client takes no second one. The TTFT limit, the idle timeout and the rate gate therefore all start after the queue.
- A caller's own deadline around a model call uses `timeout_excluding_queue`, not `tokio::time::timeout`. It moves the deadline by the time spent queued. The clock is a task-local, and `Submit` carries it to the actor's task by hand.
- A rate-floor backup takes its own slot. With every slot in use it waits, and the slow original keeps streaming.
- A queued attempt says so: `SamplingEvent::Queued`/`Dequeued` → transient `XaiSessionUpdate::RequestQueued`/`RequestDequeued` → `AcpUpdateTracker::request_queued`. The status row then reads `Queued: 7 model requests already running…` (`WaitingReason::Queued`) in place of "Waiting for response". Without it a queued turn looks like a stalled model.

## Workflow agent-concurrency notes

- `WorkflowHostParams.agent_slots` is a semaphore owned by `WorkflowManager` and shared by every run it launches (`session/workflow/manager.rs`), not one fresh semaphore per run. Up to `WORKFLOW_MAX_ACTIVE_RUNS_PER_SESSION` runs can be active at once. A per-run semaphore will let total live agent-spawned LLM requests scale with active run count instead of staying under the configured cap (`GROK_WORKFLOW_MAX_CONCURRENT_AGENTS` / `workflow_max_concurrent_agents`). The knob operators lower to stay under a hard per-host concurrent-request limit.

## Endpoint allowlist notes

- The pricing catalog is the one compiled-in endpoint (`DEFAULT_PRICING_CATALOG_URL`, modelinfo), and its request skips the allowlist (`check_catalog_url`). Nothing else prices an Anthropic model: the wire carries no cost and `/v1/models` lists no price. The proxy, the xAI API, the grok.com clients, the env crate's hosts and voice all resolve to BLANK when unconfigured. A blank URL builds no request.
- A model request reaches only an endpoint in `[endpoints] allowed_endpoints` or `GROK_ALLOWED_ENDPOINTS` (`xai_grok_extra_ca::endpoint_allowlist`), or one the user or admin config names as a URL. Every `*url` string in those config layers adds its origin (`allow_urls_written_in_config` in `util/config/campaigns.rs`), so a provider's `base_url` needs no second entry. Campaigns and remote settings add nothing: nobody local wrote them. The check runs where each request is made. Those places are `SamplingClient::new`, the model listings, the local-runtime reads, web search, image and video generation, embeddings, voice and pricing.
- A DNS resolver or a connector layer cannot enforce it. Behind a proxy the resolver sees the proxy's host, and reqwest keeps a connector's target URI private.
- `.cargo/config.toml` sets `GROK_ALLOWED_ENDPOINTS` to loopback so tests reach their mock servers. An installed binary does not get it.

## `[model_providers.<id>]` notes

[docs/model-providers-id-notes.md](docs/model-providers-id-notes.md) holds this section.

## Provider model autodetection and favorites notes

[docs/provider-model-autodetection-and-favorites-notes.md](docs/provider-model-autodetection-and-favorites-notes.md) holds this section.

## Local-runtime notes: Ollama and LM Studio

[docs/local-runtime-notes-ollama-and-lm-studio.md](docs/local-runtime-notes-ollama-and-lm-studio.md) holds this section.

# CLAUDE.md
