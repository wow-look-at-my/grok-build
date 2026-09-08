# AGENTS.md

Guidelines for autonomous coding agents working in this repository.

## Push-first workflow (most important)

CI is the source of truth and builds every pushed branch. A branch that is only built locally is not production-real, and holding work back while you verify serially wastes time when a parallel CI build can be running.

**PUSH FIRST, VERIFY AFTER.**

- As soon as a feature branch compiles (`cargo check` on the touched crate passes), commit it and push it so GitHub Actions gets a head start on the build.
- Do not let real-time verification of every edge case block the push. Push a compiling, self-consistent branch promptly, then continue verifying (unit tests, integration tests, evidence capture) while CI runs.
- Follow up on CI results and fix any failures reported for the pushed branch in follow-up commits rather than deferring the push.
- A feature branch must be created from freshly-pulled `master` and pushed with an explicit upstream: `git push --set-upstream origin <branch>` (or rely on `push.autoSetupRemote`).

## Branch hygiene

- Always branch off `master`, never off another WIP branch.
- Commit messages: concise, imperative mood, describing the change.
- Keep the working tree clean before switching context. Use `git stash` / `git stash pop` for temporary changes and restore them promptly.

## Verification

- `cargo check -p <touched-crate>` before pushing.
- `cargo test -p <touched-crate>` for the crate you changed.
- Prefer committing real tests that drive the shipped code (not mocks of the unit under test, not hand-built expected objects).
- **A web session cannot link the workspace.** `target/` reaches ~16 GB after a `cargo check` of the pager, against a ~12 GB session disk allowance, so `cargo build -p xai-grok-pager-bin` runs the container out of space. Check the crate, run that crate's tests, push, and let CI produce the binary.
- `protoc` is missing from the image and the `bin/protoc` dotslash shim cannot run either, so any build that reaches `xai-grok-tools-api` dies in its build script. Run `apt-get install -y protobuf-compiler` first.

## `--sandbox` jail notes

- A bare `--sandbox` execs this process into `bwrap` (Linux) or `sandbox-exec` (macOS) as the first statement of `main()` (`xai-grok-sandbox/src/jail.rs`). `--sandbox <profile>` keeps its older meaning and builds no jail. That is why the flag became value-optional (`num_args = 0..=1`) instead of a second flag nobody finds.
- The jail is planned off the RAW argv, not off `PagerArgs`. Precedence IS the command-line order. clap collects `--ro` and `--rw` into two separate `Vec`s, which loses how they interleaved.
- Bind order enforces precedence. The order is: the read-only system base, the user mounts as given, then `$GROK_HOME`. Bubblewrap applies binds in order and a later bind covers an earlier one. SBPL gives the last matching rule. `$GROK_HOME` is last on both, so no `--ro` takes it away.
- `/run` and `/var` are in the read-only base for one reason. On a systemd host `/etc/resolv.conf` is a symlink into one of them, and a jail without them resolves no name.
- A working directory nothing binds is refused before the exec. Bubblewrap answers that case with a bare chdir error. That error reads as a broken sandbox and not as a missing `--rw .`.
- Seatbelt confines WRITES only. The profile is `(allow default)` plus `(deny file-write*)`, so `--ro` means "not writable" there and reads stay open. Linux confines both.

## The darwin binary is compiled on Linux and linked on macOS

- A macOS runner bills at ten times the Linux rate, so `build-darwin-objects` (ubuntu-22.04) compiles every crate for `aarch64-apple-darwin`, and `link-darwin` (macos-14) runs one `cc`. That second job is the only macOS minute this workflow spends.
- Compiling for darwin on Linux works. Linking does not. Every cross-linker that reads the Apple SDK also rewrites the search paths rustc passes. Each build script's own static library then drops out of the link: aws-lc, ring, jemalloc, libgit2, the tree-sitter grammars.
- So `xai-darwin-link` stands in as rustc's linker and records the command instead of running it. It copies every input into a bundle, because rustc deletes its temporary object directory the moment the linker returns.
- Paths in the recorded list are written as `@BUNDLE@` and `@OUT@`. The replay host mounts the bundle somewhere else, and `ci/darwin-relink.sh` substitutes both.
- zig compiles the C in the build scripts, through `CC_aarch64_apple_darwin` and its siblings. It never links. The macOS SDK is still needed for Apple headers such as `CoreServices`.
- `round_trip.rs` drives the recorder and the replay script for the HOST target and runs the binary that comes out. A Linux runner cannot execute a Mach-O binary. This is the only place the replay path is covered before it reaches a Mac, and it caught the reader dropping the last argument.

## CI-status feature notes

- The GitHub CI-status dot lives in `crates/codegen/xai-grok-pager/src/ci_status.rs` (pure `gh` invocation + tri-state mapping + HSV-value animation) and is wired into the session status bar in `src/app/agent_view/render.rs`.
- The yellow "in progress" dot animates its HSV value in a sine wave between 25% and 80% (see `ci_status::animate_value`).

- The dot is only realtime because three things outside the render path keep it moving. Drop any one and it freezes at its last color, silently, on exactly the idle session that is watching CI:
  - the event loop's CI poll timer (`CI_POLL_INTERVAL`) keeps polling when no frame is being drawn — the render path refreshes only on frames it draws.
  - `set_change_notifier` gives the poller a way to ask for one repaint, and only when the color actually changed.
  - `ci_dot_animating` makes `tick_demand` report Slow while a run is in flight, which is what supplies the frames the pulse animates over.

## `/debug` feature notes

- `/debug <question>` injects the question plus an execution-context snapshot (`slash/commands/debug_context.rs`) through `CommandResult::InjectSkill`. Only `scroll`, `fps` and `log` are reserved. Everything else is free text. So a question must never come back as an "unknown option" error again.
- Staleness is `current_exe()` versus a canonicalized `$GROK_HOME/bin/grok`. `current_exe()` resolves the symlink at exec time, so after an update the two disagree and the block says the running process is not what is on disk. Both sides must stay canonicalized or every symlinked install reads as stale.
- `GROK_*`/`XAI_*` values whose NAME looks like a credential are withheld — the prompt leaves the session and lands in the model's transcript.

## Shift+Tab mode ring notes

- The ring is Plan → Auto → Always-Approve → Orchestrator → Explore → Plan (`dispatch_cycle_mode_inner` in `app/dispatch/modes.rs`). Its last two stops are agent IDENTITIES, not permission modes: they rebuild the agent (`handle_session_mode` → `handle_rebuild_agent_for_definition`) and must leave the permission mode exactly as they found it.
- Entering Orchestrator used to call `set_yolo_mode_inner(app, false)`, so cycling to it silently re-armed the approval prompt while the banner only said "Orchestrator". A subagent inherits `ctx.yolo_mode` from its parent, so that also re-armed it for everything the orchestrator delegates — the exact work nobody is watching.
- Closing the ring (past the last identity stop) DOES drop yolo before entering Plan. Plan+yolo matches no arm of the `(in_plan, in_auto, in_yolo)` match, so leaving it set sends the next press into the catch-all and lands on Normal instead of Auto.
- The composer flag row is additive, so an orchestrating yolo session correctly reads `always-approve · orchestrator` (`agent_view/render.rs`).

## `/todo` capture feature notes

- `/todo <request>` rides the `/btw` path, not the prompt queue: `Action::SendTodo` → `x.ai/todo` → `SessionCommand::TodoCapture`, spawned on the session's LocalSet (`session/acp_session_impl/todo_capture.rs`). The running turn is never interrupted. And the parent conversation is never mutated — the capture agent works from a snapshot of it.
- Appending to the todo list is its only permitted mutation. And the prompt is not what enforces that. Every tool call goes through `capture_action`: only read kinds run, and `todo_write` is rewritten by `add_only_todo_args` before dispatch — fresh `capture-`-prefixed ids, status pending, `merge` forced on. A `merge: false` replace, a status flip, and an edit of an existing item all arrive as content and leave as an append.
- It ships the main turn's full tool list even though it honors a fraction of it. The list serializes into the cached prefix, so trimming it will cost the whole conversation's prompt cache and buy nothing the dispatch gate does not already guarantee.
- The append runs through the session's own `todo_write` rather than writing `TodoState` directly, which is what makes the item persist, reach the client as a `Plan` update, and show up in the next turn's todo-gate reminder the same way one the main agent wrote does. That path is `todo_write`-specific: opencode's `todowrite` gives its items no ids and identifies them by text alone. So a capture cannot address the item it adds (or place it at the front for `/TODO`). The run fails loudly with `UnsupportedTodoTool` rather than writing through semantics that cannot express that append.
- Nothing in the loop compares against the literal `todo_write`. A harness preset renames tools per provider (`name_override`). And the model calls the renamed one. So the tool is resolved by kind and identified by NAMESPACE (`resolve_capture_todo_tool`) — the namespace is what separates the merge-capable grok_build tool from opencode's replace-only one, and it survives a rename. Item contents come back through `bridge.try_parse`, which reverse-maps renamed parameters too.
- Each turn appends `response.items` verbatim, the way the main turn records a response, never a synthesized assistant message: the Responses API rejects a continuation whose reasoning items are missing, and a hosted search's items have to ride along for the next request to make sense. Reasoning is stripped only where the backend requires it (Messages), on the loop's turns as well as the snapshot.
- The append-only guarantee is covered end to end, not just at the sanitizer: `acp_session_tests/todo_capture_e2e_tests.rs` runs the real loop against a scripted model whose `todo_write` call asks for a replace, over the main agent's own id, marking it completed. Verified red without `add_only_todo_args` (the seeded item comes back `Completed`).
- Provider-shaped failures the loop absorbs rather than dying on: transient 5xx and overloads (the `/btw` retry budget, now shared in `side_call.rs`), empty or concatenated-JSON tool arguments (`parse_tool_arguments`, mirroring the main turn), a model answering in prose instead of calling the tool (one nudge plus a write-only retry after the turn budget), and a context window too small for the conversation (`budget_instruction_items` fits the snapshot, with `LOOP_GROWTH_RESERVE_TOKENS` held back for the loop's own turns).
- The capture agent's conversation is persisted to `{session_dir}/todo-captures/todo-<uuid>.jsonl` so a missed `todo_write` is diagnosable. `NothingAdded` names that path. "Try sending again" is not the only residue.
- `/TODO` is the urgent spelling: its items go to the front of the list. Only the exact all-caps name counts (`is_urgent_token` in the pager's `slash/commands/todo.rs`) — `/Todo` and `/ToDo` are shift-key noise, not a request to jump the queue. Command resolution is case-insensitive, so the typed case reaches the command only through `SlashCommand::run_with_token`.
- The front of the list is `prepend` on `TodoWriteInput`, which is `#[schemars(skip)]`: the model cannot reach it, and the serialized tool list is unchanged, so the conversation's prompt cache survives the field. Prepend places new items only — an id already on the list keeps its position. So this is not a way around the append-only rule.
- A landed capture delivers a `<system-reminder>` to the main agent saying the user assigned the items. The list carries no provenance, so the agent read an item it did not write as somebody else's idea and cancelled it as out of scope. `/todo` reports the count only.`/TODO` names the items, because they are the next thing the agent does.
- The capture's tasks-pane row is kept, finished, rather than removed (`finish_todo_capture_ui`). The row is what holds the streamed transcript (`SessionUpdate::TodoCaptureProgress`, stamped with the client-minted `capture_id` that names the row), and removing it is what made opening the row show a blank window.

## The todo list cannot be discarded or overwritten

- A todo is the user's. Nothing can delete one: an item leaves the actionable set only by becoming `Completed` or `Cancelled`, both of which name it by id. Text is changed by sending that id with new content.
- Every `todo_write` is a merge, and an item the call omits survives with its status untouched. `merge: false` used to clear the list and keep only what the call resent, which is how a status update that forgot the flag erased the user's list.
- `merge` is still accepted on the wire and ignored, and is `#[schemars(skip)]` now that both values behave the same — advertising it will describe a choice the tool no longer offers.
- `TodoState` has no `clear` and no remove of any shape. The guarantee lives in the data structure so a later caller cannot reach around it.
- opencode's `todowrite` sends a whole list with no ids, so it merges by ITEM TEXT, not by position. Position is not identity: keying on it let a reordered or shorter list write one row's text over another's, which loses work as surely as a delete.

## Cost-indicator feature notes

- Per-message cost rides `XaiSessionUpdate::ResponseCompleted.cost_usd_ticks`, one per model call, and the pager attaches it to the message that call streamed (`AcpUpdateTracker::set_response_cost`). `TurnCompleted`'s prompt-scoped cost is the fallback for an agent that prices only whole turns. It stands down for any prompt a response already priced.
- The session total is the agent's own ledger (`ResponseCompleted`/`TurnCompleted.session_cost_usd_ticks`), not a sum over scrollback: rewound and never-rendered spend is real. The scrollback sum survives only as the fallback for an agent that reports no total.
- `ResponseCompleted` is the one buffered xAI update that is persisted — it is the only carrier of a message's cost, so a reload replays it and each message keeps its price. The indicator counts THIS run's spend: the agent's ledger is in-memory and restarts at reload. So a replayed total is not adopted and the scrollback sum stops being a valid fallback once anything priced is replayed (`AcpUpdateTracker::scrollback_sum_is_this_run`).

- The Messages backend takes a price off the wire when one is there: `MessagesUsage`/`MessageDeltaUsage` carry `cost_in_usd_ticks` (alias `cost_usd_ticks`) and the USD-float `cost`, read on `message_start` and every `message_delta` with the Chat Completions precedence — ticks over float, a zero is unbilled, and a later silent event never erases a reported price. Anthropic itself prices nothing. So that path stays `None` and the shell's `compute_cost_ticks` fallback derives one from the model's pricing.

- The per-message cache-hit-percent indicator reads the same `ResponseCompleted.usage` the cost indicator does (`AcpUpdateTracker::set_response_cache_hit`). It renders on its OWN reserved row below the content instead of widening the cost/timestamp gutter further (`EntryRenderer::cache_hit_reserved_rows`).

## Stream-timing notes

- `itl_intervals_ms` truncates every gap to whole milliseconds, so a stream above ~1000 chunks/s reads as a run of zeros and `itl_p50_ms` reports 0 for one that stutters. `InferenceLatencyStats.chunk_offsets_us` keeps each content chunk's arrival offset from `stream_start` in microseconds instead, off the `Instant`s all three backend streams already record.
- `GROK_LOG_STREAM_TIMING=1` adds it to `shell.turn.inference_done` in `~/.grok/logs/unified.jsonl`. Opt-in: it is one number per chunk on a log that is otherwise one line per model call. The gate is read once per process (`inference_metrics::log_stream_timing`), so one run's entries agree.
- The offsets are client-side SSE-parse times, so transport jitter is in them. They are not a measurement of server decode.

## Thinking-signature notes

- The Messages API verifies a thinking block's `signature` against the model that minted it, so replaying one to any other model is a 400 ("Invalid `signature` in `thinking` block") on every turn the block stays in history. It cannot be re-minted. So the block is what gives.
- `build_messages_request` reads a `Reasoning` sibling's origin off the `Assistant` item behind it (`model_id`). An alias, the dated snapshot it answers as, and a gateway's routing prefix are one model (`same_model`). An item with no recorded `model_id` is replayed as before.
- A switch only costs the thinking when a signature is in play (`thinking_is_foreign`): a signed block cannot cross one, and a model that signs rejects an unsigned block just as hard. Thinking that is plain text on both sides is nothing either end verifies. So it is replayed untouched. Whether the target signs is read off the conversation — a block it signed earlier in this one (`target_signs_thinking`) — and no evidence reads as unsigned.
- That guess, and history predating the check, are why the sampler also treats the 400 as recoverable: `RetryDecision::RetryWithReasoningStrip` drops the replayed reasoning and retries once, so history predating the check is not dead-ended.
- A conversation that ends mid-tool-loop on a turn that lost its thinking to the rule above goes out with thinking off entirely (`open_tool_loop_lost_its_thinking` asks the same predicate, so a loop that kept its thinking keeps thinking on): a provider validates the thinking of the tool-calling turn it is continuing, and a config-less thinking block is rejected in turn. Reasoning effort is untouched and the next turn pairs normally.

## Tool-call provider-field notes

- A tool call carries keys this client only relays: `extra_content` (Google's spelling) and `provider_specific_fields` (a translating gateway's). Gemini 3 rejects a replayed function call whose thought signature is missing, and that signature reaches an OpenAI-shaped client only inside one of them.
- `ToolCall::vendor` holds them from the response — including off the streaming chunk that opens the call, which is where Gemini puts the signature — and `ToolCallRequest` flattens them back onto the replay. Nothing reads them: verbatim is the only form the provider accepts.
- The allowlist (`TOOL_CALL_VENDOR_KEYS`) is what keeps response-shaped bookkeeping out of the request. A provider that sends none leaves the map empty, and an empty map flattens to nothing, so its requests are unchanged.

## Goal-planner cancellation notes

- Only steering replans. `run_goal_planner_attempt` returns `Steered` whenever there is any, so an `Interrupted` reaching the loop is a bare cancel and is terminal — retrying one spawned four dead planners in 2.3 s before the attempt cap paused the goal.
- The planner runs off a slash command, not a turn, and a user Stop latches the session's Task spawns closed until a turn reopens them (`open_subagent_spawn_admission`). `maybe_run_goal_planner` reopens them itself. Without that, `/goal resume` after a Stop is rejected before a subagent exists, at latency 0, for every message the session has left.
- A pause the user asked for says so (`planner_cancelled_pause_message`). "Planning failed" on a cancel sends the reader hunting a broken planner that is doing exactly what it was told.

## Messages thinking-dialect notes

- Claude 4.6 replaced `thinking: {type:"adaptive"}` for `{type:"enabled", budget_tokens:N}`, and each generation rejects the other's spelling outright ("Input tag 'adaptive' ... does not match any of the expected tags"), so `build_messages_request` picks by model id (`speaks_adaptive_thinking`).
- The generation is parsed off the id itself (`claude_version`), because nothing else in the request carries it: both spellings the family has used are read (`claude-haiku-4-5`, `claude-3-7-sonnet`), through a gateway prefix and a snapshot stamp. A name that is not a Claude is a gateway's own model and keeps the adaptive request it has always been sent.
- `output_config.effort` is 4.6-and-later too. So the older dialect sends the effort as `budget_tokens` instead and nothing beside it. `output_config.format` is untouched — structured outputs are not what 4.6 changed.
- A budget must clear the API's 1024 floor and stay under `max_tokens`. One that cannot do both leaves thinking off with a warning, rather than sending a request the API answers with a 400.

## CI compile-cache notes

- **A bulk `actions/cache` of `SCCACHE_DIR` does not work, and was removed.** An Actions cache entry is immutable. A key naming only the lockfile is therefore written one time and frozen. Measured: the restored entry stayed at 109 MB while the store reached 1.97 GB. The disk layer served 331 hits against 2450 misses. Every run discarded the 1389 objects it had just compiled. The store is the GHA layer's own per-object entries.
- **`RUSTC: ci/rustc-gate.sh` does NOT cap cache misses.** Measured on the full workspace: 484 gate invocations. Against that, 491 non-cacheable calls and 2374 Rust misses. sccache execs the compiler it was handed only for calls it refuses to cache. A cacheable miss compiles without passing through the gate. So the semaphore caps the wrong set and the outer `-j` runs unguarded.
- A probe crate said the opposite, and it was wrong. There a cold pass reached the gate 30 times and a warm pass 21, and the difference matched the 9 misses. Do not generalise gate behaviour from a small crate.
- The gate's cap itself is sound and has a negative control. Launching 12 against 12 slots peaks at 12. Launching 12 against 3 slots peaks at 3, and all 12 still run. What is unsound is what reaches it.
- **A detached upload inherits the compile slot unless the wrapper closes it.** Bash leaves a `{var}<` redirection open after the command it is written on. A lock is released only when its last descriptor closes. So the upload held a compile slot for its whole transfer. `exec {slot_fd}<&-` after the compile block fixes it.
- **A cap that changes nothing is not the limiter.** Give every cap a second leg that raises it. Watching the peak stay put is what named the descriptor above.
- **The compile-slot picker needs no fixing.** `$$ % SLOTS` reaches full utilisation under load. Process-id collisions do not cost a slot at these sizes.
- **The restore path is not what makes a warm build slow.** A warm pass over a populated store is all wrapper and no compiler. A CI runner is a fresh VM, and its local store is empty. So every hit there is a network fetch. That is the cost worth attacking. The measurements below come from a box with more cores than the runner. They bound the wrapper. They do not bound the runner.

```
detached upload, stub upload of 1 s, 12 calls   before: 4267 ms   after: 171 ms   one call: 54 ms
  concurrent uploads, cap 3 -> cap 8            before: 3 -> 3    after: 3 -> 7
compile slots reached, 12 calls / 3 slots       3        24 calls / 8 slots      8
warm pass, 2836 entries, -j16, no cache service 12660 ms  4 ms/call  2836 hits  5672 files
```

- **A cold full build at `-j16` with no compile cache cannot finish here.** Measured 8 times out of 8, on eight separate runners: the compile is killed with exit 143 after the runner's 15 GiB is exhausted. That leg is gone from `ci.yml` because its answer is settled, not because it was inconvenient. `-j16` is usable only behind the wrapper's compile semaphore, which caps the compiles that hold memory while restores stay unpaced.
- **A cold full build exceeds `timeout-minutes: 45`.** Measured at `-j16`: build-test compiled for 1935 s and the job was cancelled 745 s into its tests. Cold compile steps were 1935 s, 1676 s, 1623 s and 1247 s.
- `workflow_dispatch` takes a `cold` input that sets `SCCACHE_RECACHE`. A cold build can therefore be timed against the same keys without touching any of them.
- A cancelled step still carries both timestamps, so its duration reads as a plausible measurement and is not one. Check `conclusion` before quoting any step time.
- `actions/cache` restores the disk level in one download before the compile. That is the only prefetch available here. An sccache key is a hash of preprocessed input. It does not exist until the build reaches that unit. So no key can be fetched ahead of need on its own.
- Actions cache entries are immutable and are evicted WHOLE. So the disk level must never be the only copy. The GHA level holds the same objects individually. A disk level that is stale, trimmed or evicted falls through to it rather than recompiling.
- Sizing is against the repository's 10 GB. `SCCACHE_CACHE_SIZE` is 2G, and 3G on the key `build-test` and `pty-e2e` share, whose store measures 2.0 GB. One generation is about 7 GB. So a lockfile bump leaves a superseded generation for LRU to drop before it reaches a live one.
- `SCCACHE_IDLE_TIMEOUT: "0"` is what makes any of this observable. The server exits after 600s idle, and the test step is a longer gap than that. The post-job `--show-stats` then reports a fresh server's zeroes instead of the build's numbers.
- The runner is an EPYC 7763 with 2 physical cores plus SMT, 15 GiB, and 337/394 MB/s sequential disk. A developer box with 4 physical cores is faster. So a local build time is optimistic and does not transfer as an equal.

## Why build-test is not on the self-hosted runner

Pointing `build-test` at `vars.CI_RUNNER` turns ~20 tests red, because they assert on host semantics the org's lean image does not provide. Measured on that runner, with unmodified test sources:

- no PID 1 that reaps orphans and no process-group signal delivery — every `*_grandchild*` case across `xai-grok-shell`, `xai-grok-test-support`, `xai-tty-utils` and the pager PTY harness (`PTY grandchild leaked after controller Drop`), plus `scope_teardown_kills_a_background_grandchild`, which hangs to the 60s timeout instead of failing.
- overlayfs reports `st_blocks=2` for every file, so `disk_usage_cmd` and `fs_size` measure ~1 KiB for anything.
- no UTF-8 locale by default, so `xai-grok-sandbox`'s `fails_closed_on_non_utf8_*` hit errno 84.

Every one of those is the test doing its job. Making them pass there means weakening what they check, so the fix belongs to the runner image (an init/reaper, a real filesystem for `/tmp`) and that image is the fleet's, not this repo's. Revisit the runner once it has one. Until then this job is `runs-on: ubuntu-latest`, which is what `master` builds green on.

## Todo-stop-gate notes

- The built-in todo gate is a participant in the turn-end STOP-HOOK gate, not a mechanism beside it (`acp_session_impl/turn.rs`, on `StopGateDecision::AllowStop`). It fires only after the user hooks allowed the stop. Its reminder rides the same `stop_hook_feedback` user message a hook block uses. It consumes the SAME `stop_continuations_this_turn` budget. So `MAX_STOP_HOOK_CONTINUATIONS_PER_TURN` is the stuck-release: a model that never engages its todos stops anyway.
- Two switches, and they are ORed, not ANDed (`todo_stop_gate_enabled`). The persisted `[ui].stop_gate_unfinished_todos` toggle ships ON and is the switch. `todo_gate.enabled` (remote `todo_gate_enabled`, or the `--todo-gate` CLI force-enable) is an opt-in on top, for a session whose toggle the user turned off. ANDing them is what shipped the feature dead: `TodoGateConfig::default().enabled` is false, so every default session took the `None` arm and the gate never ran.
- `todo_gate_applicable` is the other half and still binds. It allows no gate while the goal loop is active, because the continuation directive drives the loop there. It allows no gate for a prompt that carries no `<task_completion_discipline>` block.
- `todo_stop_gate_blocks` is pure and table-tested. The actor supplies the toggle, the shared continuation counter, and `evaluate_todo_gate` over the live todo state.

## Workflow agent-concurrency notes

- `WorkflowHostParams.agent_slots` is a semaphore owned by `WorkflowManager` and shared by every run it launches (`session/workflow/manager.rs`), not one fresh semaphore per run. Up to `WORKFLOW_MAX_ACTIVE_RUNS_PER_SESSION` runs can be active at once, so a per-run semaphore will let total live agent-spawned LLM requests scale with active run count instead of staying under the configured cap (`GROK_WORKFLOW_MAX_CONCURRENT_AGENTS` / `workflow_max_concurrent_agents`) — the knob operators lower to stay under a hard per-host concurrent-request limit.
