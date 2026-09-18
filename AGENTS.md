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
- The Seatbelt profile also needs reads of `/` itself, metadata on every grant's ANCESTORS, and a read grant on `self_exe`. Measured on macOS 26.5: with only the grants the profile used to emit, `sandbox-exec`'s own `execvp` of the target fails and the jailed process dies with SIGABRT and no output — `--sandbox` did not start at all, on any command. A `(subpath "/usr")` grant covers that tree but not `/`, and path resolution walks the whole chain; an ancestor is granted as a `literal` (never a subtree), so an unlisted path stays invisible. `tests/jail_seatbelt_e2e.rs` launches the real jail and is the only thing that can catch this.
- `GH_HOST` set to anything but the repo's own remote host breaks every `gh` call that relies on repo autodetection ("none of the git remotes configured for this repository correspond to the GH_HOST environment variable"), so the dot goes dark — including in a `--sandbox` session, because the host worker inherits the variable. Pass `--repo <owner>/<name>` (derived from the git remote) instead of relying on discovery, or the dot works only for sessions whose `GH_HOST` is unset.
- The CI host worker's fd rides on the PLAN (`JailPlan::ci_host_fd`) so each backend builder emits it among its own options: bwrap as `--setenv GROK_CI_HOST_FD <fd>` BEFORE the `--` program separator, Seatbelt as a command env. Appending it to the finished command put it after bwrap's `--`, where it is argv for the jailed binary rather than an option — the env var never arrived (so the dot reported no CI under `--sandbox`) and the pager started with three stray arguments.

## The darwin binary is compiled on Linux and linked on macOS

- A macOS runner bills at ten times the Linux rate, so `build-darwin-objects` (ubuntu-22.04) compiles every crate for `aarch64-apple-darwin`, and `link-darwin` (macos-14) runs one `cc`. The other macOS job is `test-darwin-sandbox`. It runs `cargo test -p xai-grok-sandbox` natively, because the jail and the CI host worker have a Seatbelt half that only a Mac executes. That crate is small. The job stays cheap.
- Compiling for darwin on Linux works. Linking does not. Every cross-linker that reads the Apple SDK also rewrites the search paths rustc passes. Each build script's own static library then drops out of the link: aws-lc, ring, jemalloc, libgit2, the tree-sitter grammars.
- So `xai-darwin-link` stands in as rustc's linker and records the command instead of running it. It copies every input into a bundle, because rustc deletes its temporary object directory the moment the linker returns.
- Paths in the recorded list are written as `@BUNDLE@` and `@OUT@`. The replay host mounts the bundle somewhere else, and `ci/darwin-relink.sh` substitutes both.
- zig compiles the C in the build scripts, through `CC_aarch64_apple_darwin` and its siblings. It never links. The macOS SDK is still needed for Apple headers such as `CoreServices`.
- `round_trip.rs` drives the recorder and the replay script for the HOST target and runs the binary that comes out. A Linux runner cannot execute a Mach-O binary. This is the only place the replay path is covered before it reaches a Mac, and it caught the reader dropping the last argument.

## Release-number stamping and the shape of ci.yml

- The release number is written into the binary AFTER it links, by `xai-grok-stamp`. Nothing in a build needs a number, so `build-test`, `pty-e2e`, `test-darwin-sandbox`, `build-release` and `build-darwin-objects` all start at t=0. One `publish` job waits on every one of them, then creates the release, stamps, verifies, uploads and publishes. Reading the number with `option_env!` is what once forced the release to be created before anything was built.
- `publish` runs on `macos-14` and handles both platforms. The darwin binary must be re-signed where it is patched. Patching the ELF needs no Linux. The linux binary's own `--version` runs in `build-release` instead, on a stamped copy.
- Its uploads run at once, backgrounded in one step, through the buildhost CLI. Composite action steps cannot overlap, and `buildhost-upload-artifact` is one. The CLI chunks a body past the edge's size cap by itself.
- Each upload passes `--version` for the release the job already created. The CLI's own create call answers 409 on a version that exists, and carries on. `--draft` keeps each call from publishing the release under the other. The `buildhost-publish-release` step publishes one time at the end.
- The slot is a `#[used] static` in `xai-grok-version`: a 16-byte magic, a length byte, then 64 payload bytes. A zero length reads as unstamped. That is every local build, and every binary CI tests.
- The read is `read_volatile`. The slot is an immutable static, and the compiler knows its contents. So a plain read folds the zero length in at compile time. It never looks at what the stamper wrote.
- `version()` and `version_with_commit()` are functions for that reason. Neither can be a `const`. A `const` initialised from one fails to compile, rather than giving a wrong answer.
- The stamper refuses a binary with no slot, a binary with more than one slot, an empty version, and a version past the payload. Each of those ships a binary that reports a number the release does not carry.
- `publish-upload`'s darwin leg runs on macOS. `ld` leaves an ad-hoc signature on an arm64 Mach-O, and patching bytes invalidates it. The kernel then kills the binary on exec. `codesign --force --sign -` is what puts a signature back. Its `--version` check is also the first time anything runs the darwin binary.
- A test binary is never stamped, so `is_release_stamped()` is false throughout the suite. That is what makes folder-trust's local-build arm testable without an env override.

## CI-status feature notes

- The GitHub CI-status dot lives in `crates/codegen/xai-grok-pager/src/ci_status.rs` (pure `gh` invocation + tri-state mapping + HSV-value animation) and is wired into the session status bar in `src/app/agent_view/render.rs`.
- The yellow "in progress" dot animates its HSV value in a sine wave between 25% and 80% (see `ci_status::in_progress_dot_color`). The phase is WALL-CLOCK time (`pulse_elapsed`, one `CI_PULSE_PERIOD` per breath), never the frame tick: the loop's cadence moves with what the UI is doing (83 ms on an idle screen, ~30 fps while streaming) and a tick only advances on a frame that was drawn, so a tick-counted pulse breathes faster the busier the screen is.
- A `--sandbox` session cannot spawn `gh` in the jail. The host worker (`xai-grok-sandbox/src/ci_host.rs`) answers fixed request shapes over the inherited fd: `gh-status` feeds the dot, `gh-pr` feeds the shell's `x.ai/pr/status` (`extensions/pr.rs`). `gh pr checks` puts its verdict in the exit code (1 failed, 8 pending) and prints the list either way, so both paths accept those codes.

- The dot is only realtime because three things outside the render path keep it moving. Drop any one and it freezes at its last color, silently, on exactly the idle session that is watching CI:
  - the event loop's CI poll timer (`CI_POLL_INTERVAL`) keeps polling when no frame is being drawn — the render path refreshes only on frames it draws.
  - `set_change_notifier` gives the poller a way to ask for one repaint, and only when the color actually changed.
  - `ci_dot_animating` makes `tick_demand` report Slow while a run is in flight, which is what supplies the frames the pulse animates over.

## CI pipeline notes: the `gh` host worker, the `ci` tool, and the CI stop gate

- The unsandboxed host worker (`xai-grok-sandbox/src/ci_host.rs`) is the only way anything in a `--sandbox` session reaches `gh`. A jail re-execs the whole binary. A `gh` spawned from inside it reaches neither the host credentials nor the network. The host starts the worker moments before the re-exec and hands it in as an open socketpair fd.
- That fd is created close-on-exec. The jail is entered by exec. So `spawn_ci_host` clears `FD_CLOEXEC` on it (`inherit_across_exec`). Without that the jailed pager reads a dead fd off `GROK_CI_HOST_FD` and the dot never shows. `worker_fd_survives_an_exec` is the guard.
- A `gh` that fails is never "no runs". `ci::fetch_runs` answers `Err(CiQueryError)` with what `gh` said. The tool reports that as its error. A dead token or a rate limit then reaches the model as such, not as "nothing has been pushed". The stop gate reads the same `Err` as "cannot tell" and allows the stop.
- The worker serves `gh-status <branch>` for the dot and `gh <json argv>` for an allowlisted run. `ALLOWED_COMMANDS` and `ALLOWED_FLAGS` keep the surface read-only. The check runs on the WORKER. A jailed session that writes its own request line gets the same refusal. `gh api` is admitted because the flag allowlist refuses every flag that carries a method or a body.
- One connection, one mutex (`host_stream`). The dot polls off a blocking thread while the `ci` tool runs its own queries. Callers that write at the same time interleave their requests and read each other's answers. `concurrent_callers_never_read_each_others_answers` covers it.
- The run-to-state reduction lives in `ci_state.rs`. The dot and the tool share it. So the two cannot disagree about what red means.
- Only the newest run per workflow counts. A push cancels the run in flight. A cancelled run reads as a failure. Folding the raw list therefore leaves a branch red forever after its second push.
- The `ci` tool (`xai-grok-tools/.../grok_build/ci/`) is the model's half: `status`, `runs`, `wait`, `logs`, `checks`. `logs` defaults to the newest FAILING run, not to the newest run. A `wait` that runs out of budget reports the state it last saw. A timeout therefore reads as "still running" and not as a broken tool.
- The CI stop gate (`acp_session_impl/stop_gate.rs`) is the enforcement half. It is a participant in the turn-end STOP-HOOK gate, not a mechanism beside it. It fires only after the user hooks allow the stop. It consumes the SAME `stop_continuations_this_turn` budget. Its reminder rides the same `stop_hook_feedback` user message. So `MAX_STOP_HOOK_CONTINUATIONS_PER_TURN` is the stuck-release: a model that cannot get CI green stops anyway.
- Only RED blocks. Green, no runs, and a run still in flight each allow the stop. A gate on yellow spends the whole continuation budget on a wait for a verdict. And a repository with no workflows then never ends a turn.
- The gate is off for a subagent. A subagent does not own the branch. Sending one back over a failure its parent pushed has it fixing work it cannot see.
- The switch is the persisted `[ui].stop_gate_ci_failing` toggle, default ON. The gate reads it before the `gh` call. So a session that turns the gate off spends nothing on it per turn end.

## `/debug` feature notes

- `/debug <question>` injects the question plus an execution-context snapshot (`slash/commands/debug_context.rs`) through `CommandResult::InjectSkill`. Only `scroll`, `fps` and `log` are reserved. Everything else is free text. So a question must never come back as an "unknown option" error again.
- Staleness is `current_exe()` versus a canonicalized `$GROK_HOME/bin/grok`. `current_exe()` resolves the symlink at exec time, so after an update the two disagree and the block says the running process is not what is on disk. Both sides must stay canonicalized or every symlinked install reads as stale.
- `GROK_*`/`XAI_*` values whose NAME looks like a credential are withheld — the prompt leaves the session and lands in the model's transcript.

## Shift+Tab mode ring notes

- The ring is Plan → Auto → Always-Approve → Orchestrator → Explore → Plan (`dispatch_cycle_mode_inner` in `app/dispatch/modes.rs`). Its last two stops are agent IDENTITIES, not permission modes: they rebuild the agent (`handle_session_mode` → `handle_rebuild_agent_for_definition`) and must leave the permission mode exactly as they found it.
- Entering Orchestrator used to call `set_yolo_mode_inner(app, false)`, so cycling to it silently re-armed the approval prompt while the banner only said "Orchestrator". A subagent inherits `ctx.yolo_mode` from its parent, so that also re-armed it for everything the orchestrator delegates — the exact work nobody is watching.
- Closing the ring (past the last identity stop) DOES drop yolo before entering Plan. Plan+yolo matches no arm of the `(in_plan, in_auto, in_yolo)` match, so leaving it set sends the next press into the catch-all and lands on Normal instead of Auto.
- The composer flag row is additive, so an orchestrating yolo session correctly reads `always-approve · orchestrator` (`agent_view/render.rs`).

## `/goal` role-model notes

- Every `/goal` role (planner, strategist, skeptic panel) inherits the session's current model unless something pins it. Precedence is `[goal].use_current_model_only` (kill switch, wins over everything) > a local `[goal]` pin > a remote-pushed pin > inherit.
- A remote pin applies only with `[goal].follow_remote_role_models` (env `GROK_GOAL_FOLLOW_REMOTE_ROLE_MODELS`), default off. A server-side pin silently replaces the model the user picked. Nothing local reports which model a role ran on.
- The opt-in itself is local only. A remote-controlled switch for whether to obey remote pins grants back what the default withholds.

## Goal-plan-to-todos notes

- The implementing session no longer transcribes the plan into its todo list. The planner lists the plan's work on its OWN todo list, and the harness puts those items on the session's list as the plan is published (`apply_planner_todos` in `acp_session_impl/goal_support.rs`, called from the `Planned` publish branch of `maybe_run_goal_planner`, before the goal-start reminder is rendered).
- The planner is told to make that call — `## Todo list — REQUIRED` in `templates/goal_planner_prompt.md`, with the tool named by `{TODO_TOOL}` (`RoleToolNames`, resolved per harness so a `name_override` is honored). A child session keeps its OWN `State<TodoState>`, so a planner-issued `todo_write` lands on the child's list rather than the session's: `run_shell_child` reads that list into the new `SubagentResult.todos` before the child is torn down, and it rides back through `GoalPlannerSpawner` / `GoalPlannerOutcome::Planned`, which is the only route by which the session can see it. That round trip is what makes the session's list the planner's own items.
- The harness never mines the plan prose for items. A planning run that named none seeds nothing and leaves `plan_todos_seeded` false, so an unfollowed instruction degrades to "the main agent keeps its own list" instead of machinery inventing work. `a_planner_that_named_no_items_leaves_the_list_untouched` is the guard.
- Once per goal, append-only. `GoalOrchestration::plan_todos_seeded` is claimed under the tracker lock before any I/O, and the append is deduped against the live list by content. Existing items keep their id, text and status; a retry, a resume or a direct re-entry adds nothing.
- The append goes through the session's own todo path (`append_capture_todos`, `add_only_todo_args_with_prefix` with a `plan-` id prefix), so the persisted state and the client's `Plan` update move exactly as a model-written `todo_write` does. Seeding is best-effort: no append-capable todo tool, or a failed append, logs and returns, and never fails the goal.
- The child's list is read through the shared workspace handle (`WorkspaceOps::workspace_handle` → the session's `toolset().resources`), so it needs LOCAL mode. A proxied session (the workspace server owns sessions) has no handle here, the read returns empty, and the feature degrades to the main agent keeping its own list. Nothing breaks; nothing is populated either.
- `Plan: <path>` still renders on every plan-aware reminder — only the manual seed-todos directive is gone, replaced by a statement that the steps are already on the list.
- A fail-closed planner publishes no plan, so nothing is seeded. Red/unseeded is the honest state there.

## Verification does not widen the goal

- A `## Verification plan` step reads back what the goal built. It is not a permit. The implementer read "do X to confirm Y" as an instruction to do X. A planner-invented check then became an action on a system nobody put in scope. Every place that demands verification says so now. Those are the planner prompt's `## Verification plan` contract, `goal_rules.md`'s VERIFY AS YOU GO, `goal_plan_block.md`, and the per-turn continuation directive.
- The planner is told to prefer reading what the work already produced over operating anything. Files, logs, hashes, build output and source are what it reads. That is the whole mechanism. There is no label grammar and no validator. An earlier attempt added a reach DSL, a keyword list and a reject-and-retry loop to a planner prompt that is already long. That buys rigidity rather than scope discipline.

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

## Output-budget notes

- A provider charges the requested output against the same window as the prompt. A conversation that is under the window on its own can still put the REQUEST over it. 737_857 input tokens plus a 262_144 output budget is 1_000_001 against a 1M window. The server answers 400. `max_output_tokens` is not a free parameter of the request. It is whatever the window has left.
- `xai_token_estimation::fit_output_tokens` is that arithmetic. `ConversationRequest::fit_output_budget` applies it. `build_conversation_request` (chat-state) fits against the tracked total. That total is the provider's own usage for the last response plus the estimated delta, which is the best number this process has.
- `apply_conversation_defaults` (sampler) fits again, against its own bytes/4 estimate. The sampler's DEFAULT budget is what a caller that sets none sends. Every backend converter reads the field from there. This pass only cuts further.
- Both fit against `window_less_estimate_slack`, which holds back 1% of the window. Every prompt count here is an estimate somewhere. One token low is a rejected request.
- The budget never goes below `MIN_OUTPUT_TOKENS`. A prompt that leaves less room than that is over the window. The provider's own overflow error is the honest report of it. The compaction ladder answers that error.
- `check_preflight_overflow` and `should_compact_on_error` measure against the window less that same floor. A prompt with no room for an answer needs compaction, not a 1024-token reply.
- `should_compact_on_error` also takes the server's own context-length message as decisive. Its tokenizer is the one that counts. A rejection that names the context length is never a turn to hand back to the user.

## Model-pricing resolution notes

- `model_pricing::resolve` (`xai-grok-shell/src/agent/model_pricing.rs`) answers the `compute_cost_ticks` fallback for an endpoint that reports no price. It reads `[model.<id>].pricing` from config first. A price the user wrote is the price, and the catalog never overrides it. `config::resolve_configured_model_pricing` is that first tier.
- The catalog is modelinfo. One model's document is at `<catalog_url>/v1/models/<model id>`. `[pricing].catalog_url` moves it and `[pricing].lookup_enabled = false` keeps the session off the network. The whole-catalogue `/v1/models` route answers with about 20 MB, so nothing fetches it.
- `resolve` runs on the turn path and is sync. So it never waits on the network. A model with no fresh cache entry answers as unpriced for that call and starts a background fetch. The price lands for the next call. `in_flight` holds one fetch per model. A second turn therefore starts no second request.
- The cache is `$GROK_HOME/model_pricing_cache.json`, one entry per model. An absence is cached too, with a shorter TTL, or every turn on an unpriced model re-fetches. A failed lookup is NOT an absence and is not cached. Caching one pins an outage into the catalog for the whole TTL.
- A document that prices no tier is recorded as an absence. Recording it as an all-zero price claims a price the catalog never gave.

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

- Nothing replans. A Send Now delivers its text to the planner already running (`SubagentEvent::Interject`, routed by the coordinator id the spawn publishes on the goal tracker) instead of cancelling it, so an `Interrupted` reaching the loop is a bare cancel and is terminal — retrying one spawned four dead planners in 2.3 s before the attempt cap paused the goal.
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
- **A lock a forked child needs is taken before the fork.** The upload took its shared lock inside the child. A drain that arrived between the wrapper exiting and the child locking returned early. The entry then reached no service, and the next run recompiled it while the warm leg called it a hit.
- **A fetch that lands and fails to restore is not a miss.** Both spell compile it again. Only one says the service lacks the entry. `remote-restore-failed` is counted apart so a warm leg names which end broke.
- **The index fetch and publish have an offline test.** A stub service backed by a directory drives `measure-compile.sh` through both legs. That is what found the drain race. It needs no network and no runner.
- **A cap that changes nothing is not the limiter.** Give every cap a second leg that raises it. Watching the peak stay put is what named the descriptor above.
- **The compile-slot picker needs no fixing.** `$$ % SLOTS` reaches full utilisation under load. Process-id collisions do not cost a slot at these sizes.
- **A restored artifact keeps its content and loses its mode.** binpazer stores payloads and models no permission. The names block carries the mode beside the name for that reason. Without it a build script's own binary comes back unexecutable. Cargo then answers `Permission denied ... (never executed)` on a hit. A local hit hardlinks and keeps the bit. So only an entry from the service was affected.
- **`diff -r` cannot see a lost permission.** The round-trip probe compared content alone. It passed every run while that defect shipped. It now packs an executable and compares the modes apart from the content.
- **The index key names the technique.** A cache entry is immutable, so one shared key is frozen by whichever leg publishes first. A leg that runs no wrapper stores nothing. Its empty index reached the warm pkgcache leg, which held every key to be absent and recompiled the workspace. A leg with no wrapper now publishes no index at all.
- **A warm leg that serves nothing must fail.** It reported success, because the only guard read the throttle counter. Its wall time then reads as the technique's warm cost and is a cold build. The guard now reads the hit counters. A cold leg that cannot publish its index fails for the same reason.
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

Every one of those is the test doing its job. Making them pass there means weakening what they check, so the fix belongs to the runner image (an init/reaper, a real filesystem for `/tmp`) and that image is the fleet's, not this repo's. Revisit the runner once it has one. Until then this job is `runs-on: ubuntu-22.04`, like every other Linux job in the workflow, which is what `master` builds green on.

## Todo-stop-gate notes

- The built-in todo gate is a participant in the turn-end STOP-HOOK gate, not a mechanism beside it (`acp_session_impl/turn.rs`, on `StopGateDecision::AllowStop`). It fires only after the user hooks allowed the stop. Its reminder rides the same `stop_hook_feedback` user message a hook block uses. It consumes the SAME `stop_continuations_this_turn` budget. So `MAX_STOP_HOOK_CONTINUATIONS_PER_TURN` is the stuck-release: a model that never engages its todos stops anyway.
- Two switches, and they are ORed, not ANDed (`todo_stop_gate_enabled`). The persisted `[ui].stop_gate_unfinished_todos` toggle ships ON and is the switch. `todo_gate.enabled` (remote `todo_gate_enabled`, or the `--todo-gate` CLI force-enable) is an opt-in on top, for a session whose toggle the user turned off. ANDing them is what shipped the feature dead: `TodoGateConfig::default().enabled` is false, so every default session took the `None` arm and the gate never ran.
- `todo_gate_applicable` is the other half and still binds. It allows no gate while the goal loop is active, because the continuation directive drives the loop there. It allows no gate for a prompt that carries no `<task_completion_discipline>` block.
- `todo_stop_gate_blocks` is pure and table-tested. The actor supplies the toggle, the shared continuation counter, and `evaluate_todo_gate` over the live todo state.

## `send_message` notes

- One tool carries both directions (`grok_build/send_message/`). `to` is a subagent id from `task` to reach a child. `to` is `parent` to reach the session that spawned this one. The recipient reads the text as a mid-turn user message. It keeps the work it is streaming and reads at its next drain point.
- The parent-to-child leg is `SubagentEvent::MessageChild`, NOT the host's `SubagentEvent::Interject`. `MessageChild` is scoped by `parent_session_id`. A child of another session answers `NotOwned` instead of taking a stranger's instruction. Every path also answers on a oneshot. The model reads `Delivered`, `Queued`, `NotOwned` or `NotFound` in place of a silent success over a dropped message. `Interject` stays unscoped and silent because the user owns it.
- A child that has not started holds the message in `held_interjections` and answers `Queued`. The held text goes out on `Started`, in order, through the path the user's interjections use.
- The child-to-parent leg is a host-supplied closure, `ParentMessenger`. It is not a channel the tool can address. The host owns the provenance line. A child session does not know which subagent it is. Without that line the parent reads an unattributed message as the user's own. The closure wraps `ctx.parent_cmd_tx` and sends `SessionCommand::InterjectWithoutCancel`.
- `ParentMessenger` rides `AgentRebuildSpec`. One `update_resource` after spawn is not enough. An agent rebuild builds a fresh tool bridge. A mode switch or a model switch triggers one. A resource registered one time is gone after that rebuild. The child then loses its way to answer its parent. Nothing reports the loss.
- `ToolKind::SendMessage` is a meta kind in `kind_allowed`. Every `CapabilityMode` allows it. The tool writes nothing. And a read-only explorer still has to answer the session that spawned it.

## History-flattening notes

- A history carries state that belongs to the provider that made it. A reasoning item's `encrypted_content`, a thinking block's signature, and a tool call's id and vendor fields are each such state. Another model refuses a request that replays them. `flatten_conversation` (`xai-grok-sampling-types/src/conversation/flatten.rs`) rewrites the conversation so none of it is left.
- Reasoning becomes a `<thinking>` assistant message. An assistant message's tool calls become `<tool_call>` blocks in its own text. A tool result becomes a `<tool_result>` user message, because no call is left to pair it with. A server-side call becomes assistant text. System and user messages pass through. Neither carries provider state.
- Every assistant message the flattening touches loses its `model_id`. The thinking-signature rules read that origin. A flattened message came from no model. A stale origin there arms those rules against text they do not apply to.
- A reasoning item with an encrypted blob and no text is dropped. Nothing in it reaches the next model. `FlattenReport.reasoning_dropped` counts it, so the one real loss is visible in the log.
- `needs_flattening` is also the loop bound. One flattening leaves nothing for a second to find. So a model that still refuses a flat history gets a terminal error that quotes what it said, in place of another resubmit.
- A model switch onto another harness sends `SessionCommand::FlattenHistory` and switches (`agent/handlers/model_switch.rs`). `MODEL_SWITCH_INCOMPATIBLE_AGENT` is dead on this path. A mid-turn rejection naming `encrypted_content` flattens and resubmits the turn instead of ending it (`acp_session_impl/sampler_turn.rs`, `SamplerFailureRecovery::FlattenAndResubmit`).
- `SessionCommand::RebuildAgentForDefinition` carries `zero_turn`. That flag gates conversation surgery which assumes `conversation[1]` is the synthetic zero-turn prefix. A mid-session switch reaches this path now. A `true` there writes over the session's first real user message.
- Thinking a model cannot verify rides as text, in both places that handle it: `build_messages_request` and the sampler's `RetryWithReasoningStrip` recovery (`ConversationRequest::reasoning_to_plain_text`). A block carrying only a signature has no words. That block is what goes.
- The pager keeps its `MODEL_SWITCH_INCOMPATIBLE_AGENT` handling and its `model_incompatible` flag. The shell sends neither on the recoverable paths. An older shell on the other end of ACP still can.

## Project-instruction `@import` notes

- An `@ref` in a discovered instruction file names a file to deliver (`prompt/agents_md_imports.rs`). Before it existed, this repo's `CLAUDE.md` shipped the literal line `@AGENTS.md` and none of the rules under it.
- An imported file is its OWN `AgentConfigFile`, placed right after the file that named it, rather than text spliced into the importer. That keeps the `## From:` path on every instruction. It also lets discovery's canonical-path dedup cover imports. A ref to a file discovery already found therefore adds nothing.
- The gitignore filter is discovery's, not the import path's. A ref is a deliberate instruction to read that file. Applying the filter to it makes a personal `CLAUDE.local.md` unimportable, which is the one thing people gitignore it for.
- A rule file's frontmatter is stripped from the RULE, never from what the rule imports. The import is read as written.
- `MAX_IMPORT_DEPTH` plus the seen-set bound the walk. The seen-set is what terminates a cycle. The depth cap only bounds a chain.

## Stream-interruption retry notes

- A response stream that dies mid-body has its own retry budget: `STREAM_INTERRUPT_MAX_RETRIES` = 10, on the transport path's exponential backoff (2s, 4s, 8s, ... capped at `MAX_RETRY_BACKOFF`, jittered). `SamplingError::is_stream_interrupted` names the class. `request_task` charges it to `stream_retry_count` and not to the transport budget. A dropped connection is not a server fault, and the next 5xx still needs its own retries.
- The budget is a floor as well as a cap. A model configured with `max_retries = 3` still gets these 10. Only `max_retries = 0` (observe-only) and a caller that cannot take duplicate output (`retry_only_before_output` after output) get zero, through `stream_interrupt_budget`.
- A reqwest decode failure is in that class, and `is_retryable_reqwest` answers true for it. Its Display is "error decoding response body". A body that stopped arriving mid-read is transient, so calling it fatal ends a turn on one network blip. The same failure reaches the user as `EventStreamError` when the SSE stream is what broke.

## Output-rate floor notes

- One meter serves both halves (`xai-grok-sampling-types/src/output_rate.rs`). `OutputRateGate` owns an `OutputRateMeter` and the sustained-breach state machine. `OutputRateHealth`/`classify_rate` is the reduction the indicator color, the slowdown log and the reissue all read. A second meter for the display lets the number on screen disagree with the number the gate acted on.
- The meter records BYTES. It divides one time, over the window's whole byte count. `estimate_tokens` truncates. So a stream of short chunks, estimated one at a time, reads as a total stall at any real rate. `short_chunks_are_not_rounded_away` is the guard.
- TTFT is outside all of it. The meter starts at the FIRST CONTENT CHUNK. A long prefill therefore neither depresses the rate nor counts toward a breach. A queued request is the idle timeout's business. `a_long_prefill_is_not_a_slowdown` covers it.
- `rate()` answers `None` until `MIN_DISPLAY_SPAN` of stream. Below that span one chunk's arrival jitter dominates. The quotient is then noise.
- The durations are separate knobs. `window_secs` (default 10) is what the rate is AVERAGED over. `sustained_secs` (default 10) is how long that average must stay under the floor before the request is reissued. A dip shorter than the sustained duration is a pause, not a collapsed engine.
- A short stall after a fast burst does not move the average below the floor. That smoothing is what the window is for. So a test that drives a recovering dip must outlast the window.
- The gate is ticked on a timer (`RATE_TICK`). A tick on an arriving chunk is not enough. A stream that stops dead delivers nothing to record. The tick is what turns that silence into a falling rate.
- It runs in `drive_l2` (`xai-grok-sampler/src/actor/request_task.rs`), not in a backend transform. Every backend's tokens and tool-call arguments pass through that loop, so one meter covers Chat Completions, Responses and Messages.
- Tool-call arguments count as generation. A response that collapses while it writes a large edit is the case the floor exists for. A meter that counts only text reads that case as silence.
- A breach answers `SamplingError::OutputRateCollapsed`. The request is reissued on the rate gate's OWN budget (`rate_retry_count`), the way the doom-loop recovery does. A collapsed engine therefore spends none of the transport budget. A spent rate budget disarms the gate, so the attempt completes instead of the turn dying. `retry_only_before_output` still wins: a caller that cannot take duplicate output gets the failure reported instead.
- The finest granularity available is ONE MODEL CALL. Dropping the L2 stream cancels that HTTP request. Every earlier response and tool call in the turn is already in the conversation, and stays untouched. Nothing resumes a half-written response, because no provider here accepts one back.
- Both slowdown EDGES are logged, not the breach alone (`output_rate_slowdown_start`, `output_rate_slowdown_end`, `output_rate_breached`). A dip that recovers by itself is never reissued over. Nothing else records the seconds it cost.
- `SamplingEvent::OutputRate` → `XaiSessionUpdate::OutputRate` → `AcpUpdateTracker::set_output_rate` feeds the indicator. It is transient and never persisted. A rate describes a stream in flight. A replayed one puts a stale number under an idle session. `finish_turn` clears it.
- Config: `[ui].min_output_tokens_per_sec` is the settings-modal floor, where a zero turns the gate off. `[ui].output_rate_sustained_secs` is the grace period. `[model.<id>].min_output_tokens_per_sec` overrides the floor for one model. `[output_rate_floor]` holds the window and the reissue budget. One key, one home.
- `Config::resolve_output_rate_floor` is the whole precedence. The session caches the answer in a `Cell`. It re-resolves on a model switch rather than on each turn, because the floor reads the config off disk.

## `/fork` and running subagents

- A fork copies `updates.jsonl`, and the child's load replays it. A `subagent_spawned` with no matching `subagent_finished` is therefore inherited. `prepare_replay_lines` reports it as unfinished and the child opens with that agent's row. The run itself stays the parent's, because `emit_subagent_notification` addresses the `parent_session_id` recorded at spawn. So the finish never reaches the child.
- The copy drops the records of every subagent that is still RUNNING at the fork point (`CopySessionOptions::carry_running_subagents`, default off). A subagent that already finished is history the conversation refers to. Its spawn and finish pair is copied either way. This is the same boundary the copy draws for workflow and goal projections.
- Running is decided over the lines the copy KEEPS. It is not decided over the whole source file. A `target_prompt_index` that cuts a finish away leaves a spawn the child reads as live. That spawn is dropped too.
- `/fork --agents` opts back in. The records are copied. The child's load then reconciles them the way a resumed session's are (`reconcile_orphaned_subagents_with_backend`). An agent this process still has running keeps its row. One that is gone is finished as cancelled. The child still cannot receive that run's output, because the run answers to the parent.
- The flag rides `ForkSessionRequest::include_agents` on `x.ai/session/fork` and `ResumeSessionInWorktreeRequest::include_agents` on the worktree fork. So `/fork --worktree --agents` behaves the same.

## Workflow agent-concurrency notes

- `WorkflowHostParams.agent_slots` is a semaphore owned by `WorkflowManager` and shared by every run it launches (`session/workflow/manager.rs`), not one fresh semaphore per run. Up to `WORKFLOW_MAX_ACTIVE_RUNS_PER_SESSION` runs can be active at once, so a per-run semaphore will let total live agent-spawned LLM requests scale with active run count instead of staying under the configured cap (`GROK_WORKFLOW_MAX_CONCURRENT_AGENTS` / `workflow_max_concurrent_agents`) — the knob operators lower to stay under a hard per-host concurrent-request limit.
