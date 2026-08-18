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

## Shift+Tab mode ring notes

- The ring is Plan → Auto → Always-Approve → Orchestrator → Explore → Plan
  (`dispatch_cycle_mode_inner` in `app/dispatch/modes.rs`). Its last two stops
  are agent IDENTITIES, not permission modes: they rebuild the agent
  (`handle_session_mode` → `handle_rebuild_agent_for_definition`) and must
  leave the permission mode exactly as they found it.
- Entering Orchestrator used to call `set_yolo_mode_inner(app, false)`, so
  cycling to it silently re-armed the approval prompt while the banner only
  said "Orchestrator". A subagent inherits `ctx.yolo_mode` from its parent, so
  that also re-armed it for everything the orchestrator delegates — the exact
  work nobody is watching.
- Closing the ring (past the last identity stop) DOES drop yolo before
  entering Plan. Plan+yolo matches no arm of the `(in_plan, in_auto, in_yolo)`
  match, so leaving it set sends the next press into the catch-all and lands on
  Normal instead of Auto.
- The composer flag row is additive, so an orchestrating yolo session correctly
  reads `always-approve · orchestrator` (`agent_view/render.rs`).

## `/todo` capture feature notes

- `/todo <request>` rides the `/btw` path, not the prompt queue: `Action::SendTodo`
  → `x.ai/todo` → `SessionCommand::TodoCapture`, spawned on the session's
  LocalSet (`session/acp_session_impl/todo_capture.rs`). The running turn is
  never interrupted, and the parent conversation is never mutated — the capture
  agent works from a snapshot of it.
- Appending to the todo list is its only permitted mutation, and the prompt is
  not what enforces that. Every tool call goes through `capture_action`: only
  read kinds run, and `todo_write` is rewritten by `add_only_todo_args` before
  dispatch — fresh `capture-`-prefixed ids, status pending, `merge` forced on.
  A `merge: false` replace, a status flip, and an edit of an existing item all
  arrive as content and leave as an append.
- It ships the main turn's full tool list even though it honors a fraction of
  it. The list serializes into the cached prefix, so trimming it would cost the
  whole conversation's prompt cache and buy nothing the dispatch gate does not
  already guarantee.
- The append runs through the session's own `todo_write` rather than writing
  `TodoState` directly, which is what makes the item persist, reach the client
  as a `Plan` update, and show up in the next turn's todo-gate reminder the same
  way one the main agent wrote does. That path is `todo_write`-specific:
  another harness's task-list tool (opencode's `todowrite`) replaces the list
  instead of merging, so the run fails loudly with `UnsupportedTodoTool`
  instead of writing through semantics that cannot express an append.
- Nothing in the loop compares against the literal `todo_write`. A harness
  preset renames tools per provider (`name_override`), and the model calls the
  renamed one, so the tool is resolved by kind and identified by NAMESPACE
  (`resolve_capture_todo_tool`) — the namespace is what separates the
  merge-capable grok_build tool from opencode's replace-only one, and it
  survives a rename. Item contents come back through `bridge.try_parse`, which
  reverse-maps renamed parameters too.
- Each turn appends `response.items` verbatim, the way the main turn records a
  response, never a synthesized assistant message: the Responses API rejects a
  continuation whose reasoning items are missing, and a hosted search's items
  have to ride along for the next request to make sense. Reasoning is stripped
  only where the backend requires it (Messages), on the loop's turns as well as
  the snapshot.
- The append-only guarantee is covered end to end, not just at the sanitizer:
  `acp_session_tests/todo_capture_e2e_tests.rs` runs the real loop against a
  scripted model whose `todo_write` call asks for a replace, over the main
  agent's own id, marking it completed. Verified red without
  `add_only_todo_args` (the seeded item comes back `Completed`).
- Provider-shaped failures the loop absorbs rather than dying on: transient 5xx
  and overloads (the `/btw` retry budget, now shared in `side_call.rs`), empty
  or concatenated-JSON tool arguments (`parse_tool_arguments`, mirroring the
  main turn), a model answering in prose instead of calling the tool (one
  nudge), and a context window too small for the conversation
  (`budget_instruction_items` fits the snapshot, with `LOOP_GROWTH_RESERVE_TOKENS`
  held back for the loop's own turns).

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

- The Messages backend takes a price off the wire when one is there:
  `MessagesUsage`/`MessageDeltaUsage` carry `cost_in_usd_ticks` (alias
  `cost_usd_ticks`) and the USD-float `cost`, read on `message_start` and every
  `message_delta` with the Chat Completions precedence — ticks over float, a
  zero is unbilled, and a later silent event never erases a reported price.
  Anthropic itself prices nothing, so that path stays `None` and the shell's
  `compute_cost_ticks` fallback derives one from the model's pricing.

## Thinking-signature notes

- The Messages API verifies a thinking block's `signature` against the model
  that minted it, so replaying one to any other model is a 400
  ("Invalid `signature` in `thinking` block") on every turn the block stays in
  history. It cannot be re-minted, so the block is what gives.
- `build_messages_request` reads a `Reasoning` sibling's origin off the
  `Assistant` item behind it (`model_id`). An alias, the dated snapshot it
  answers as, and a gateway's routing prefix are one model (`same_model`); an
  item with no recorded `model_id` is replayed as before.
- A switch only costs the thinking when a signature is in play
  (`thinking_is_foreign`): a signed block cannot cross one, and a model that
  signs rejects an unsigned block just as hard. Thinking that is plain text on
  both sides is nothing either end verifies, so it is replayed untouched.
  Whether the target signs is read off the conversation — a block it signed
  earlier in this one (`target_signs_thinking`) — and no evidence reads as
  unsigned.
- That guess, and history predating the check, are why the sampler also
  treats the 400 as recoverable:
  `RetryDecision::RetryWithReasoningStrip` drops the replayed reasoning and
  retries once, so history predating the check is not dead-ended.
- A conversation that ends mid-tool-loop on a turn that lost its thinking to
  the rule above goes out with thinking off entirely
  (`open_tool_loop_lost_its_thinking` asks the same predicate, so a loop that
  kept its thinking keeps thinking on): a provider
  validates the thinking of the tool-calling turn it is continuing, and a
  config-less thinking block is rejected in turn. Reasoning effort is untouched
  and the next turn pairs normally.

## Tool-call provider-field notes

- A tool call carries keys this client only relays: `extra_content` (Google's
  spelling) and `provider_specific_fields` (a translating gateway's). Gemini 3
  rejects a replayed function call whose thought signature is missing, and that
  signature reaches an OpenAI-shaped client only inside one of them.
- `ToolCall::vendor` holds them from the response — including off the streaming
  chunk that opens the call, which is where Gemini puts the signature — and
  `ToolCallRequest` flattens them back onto the replay. Nothing reads them:
  verbatim is the only form the provider accepts.
- The allowlist (`TOOL_CALL_VENDOR_KEYS`) is what keeps response-shaped
  bookkeeping out of the request. A provider that sends none leaves the map
  empty, and an empty map flattens to nothing, so its requests are unchanged.

## Goal-planner cancellation notes

- Only steering replans. `run_goal_planner_attempt` returns `Steered` whenever
  there is any, so an `Interrupted` reaching the loop is a bare cancel and is
  terminal — retrying one spawned four dead planners in 2.3 s before the
  attempt cap paused the goal.
- The planner runs off a slash command, not a turn, and a user Stop latches the
  session's Task spawns closed until a turn reopens them
  (`open_subagent_spawn_admission`). `maybe_run_goal_planner` reopens them
  itself; without that, `/goal resume` after a Stop is rejected before a
  subagent exists, at latency 0, for every message the session has left.
- A pause the user asked for says so (`planner_cancelled_pause_message`).
  "Planning failed" on a cancel sends the reader hunting a broken planner that
  is doing exactly what it was told.

## Messages thinking-dialect notes

- Claude 4.6 replaced `thinking: {type:"adaptive"}` for
  `{type:"enabled", budget_tokens:N}`, and each generation rejects the other's
  spelling outright ("Input tag 'adaptive' ... does not match any of the
  expected tags"), so `build_messages_request` picks by model id
  (`speaks_adaptive_thinking`).
- The generation is parsed off the id itself (`claude_version`), because
  nothing else in the request carries it: both spellings the family has used
  are read (`claude-haiku-4-5`, `claude-3-7-sonnet`), through a gateway prefix
  and a snapshot stamp. A name that is not a Claude is a gateway's own model
  and keeps the adaptive request it has always been sent.
- `output_config.effort` is 4.6-and-later too, so the older dialect sends the
  effort as `budget_tokens` instead and nothing beside it. `output_config.format`
  is untouched — structured outputs are not what 4.6 changed.
- A budget must clear the API's 1024 floor and stay under `max_tokens`. One
  that cannot do both leaves thinking off with a warning, rather than sending a
  request the API answers with a 400.

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
