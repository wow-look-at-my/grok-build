# Split-and-tee (experimental)

The toggle is `[ui].split_and_tee_commands`, default off, row "Split and tee commands (experimental)" in the settings modal. A session reads it when it starts (`spawn.rs`), and only for a terminal on this machine. The stage files sit next to the session's own logs, and an ACP client terminal cannot write them there.

It does things.

## 1. A joined bash call becomes one tool call per command

`cd crates; cargo build && cargo test` from the model runs as `run_terminal_cmd` calls:

| id | command | runs when |
| --- | --- | --- |
| `<id>` | `cd crates` | always |
| `<id>_split2` | `cargo build` | after the first finished |
| `<id>_split3` | `cargo test` | after the second succeeded |

- The rewrite happens in `turn.rs` before the assistant item is recorded (`split_joined_bash_calls`, `acp_session_impl/command_split.rs`). History and the pager both see the split calls, and each call gets its own result.
- The first call keeps the model's id. As a result, it takes over the row the pager drew while the model streamed the call. Provider fields (`vendor`) stay on that first call only.
- Calls from one response normally run concurrently. `execute_tool_calls_with_chains` runs every later member of a chain in its own batch, after the member before it has finished. Calls that are not in a chain keep the old behaviour.
- `&&` skips the rest of the and-list when a member fails, and `;` does not. A member that moved to the background stops the rest of its chain, because a later command will run beside it and not after it. A skipped member gets its own row and a "Not run: ..." result (`chain_step`, `skip_chain_call`).
- Only the grok_build bash tool is split (`ToolKind::Execute` + `ToolNamespace::GrokBuild`), and only when `is_background` is false. The argument names come from the template renderer, so renamed parameters still work.
- Each call runs in its own shell. The terminal keeps the cwd, exported variables, options, functions and aliases between calls, and nothing else. So `split_command_list` (`bash/command_plan.rs`) refuses any list that needs one shell. That covers an unexported assignment, `$?`/`$!` after the first command, and a stateful builtin (`set`, `source`, `exec`, `read`, ...). It also covers `||`, `&`, a heredoc, a subshell, a group and every compound command. A refused command runs unchanged.

## 2. Every stage of a pipe keeps its output

A foreground command that is exactly one pipeline runs rewritten (`plan_pipeline_capture`):

```
cargo test 2>&1 | grep FAIL | sort
→ cargo test 2>&1 | tee -- '<log>.stage1.log' | grep FAIL | tee -- '<log>.stage2.log' | sort
  __grok_rc=$? __grok_ps=("${PIPESTATUS[@]}" "${pipestatus[@]}"); printf ... > '<log>.stages'; ( exit "$__grok_rc" )
```

- `<log>` is the call's own output file, `<session>/terminal/<call id>.log`. The last stage's output is that file, as before.
- `<log>.stages` holds one exit code per process. Every stage but the last is followed by its tee, so the stage codes sit at even indices (`stage_exit_codes`).
- `|&` keeps stderr in the stage file. The terminal gets the model's own command as `display_command`, so the task snapshot and get_task_output show what the model wrote.
- Tee stops on SIGPIPE like any stage. So `cmd | head -5` still stops `cmd` early. The stage file holds what `cmd` wrote before that. Head is left in the pipe on purpose: taking it out will turn `yes | head` into a command that never ends.

## 3. A trailing `| tail` is a view

A last stage that is a bare `tail` over the pipe (`tail`, `-N`, `-n N`, `-nN`, `--lines=N`, `-n +N`) is taken out of the pipe. The command runs without it. The whole output is kept, and the result shows those lines of it. Tail reads all of its input anyway. As a result, the command costs the same. The exit code is then the real command's, not tail's. The footer says so.

## Reading it back

The result footer (`bash/capture_report.rs`) lists every stage with its exit code and size, and names the exact call:

```
[every stage of this pipe was kept]
  stage 1 `cargo test 2>&1`: exit 101, 4213 lines, 312.4 KB
  stage 2 `grep FAIL`: exit 1, 0 lines, 0 B
  stage 3 `sort`: exit 0, the output above
Read it with get_task_output("task_ids": ["<id>"], "stage": 1, "tail": 100). ...
```

`get_task_output` takes `stage`, `head`, `tail` and `grep` (`task_output/view.rs`), and these work with the mode off too. They read the whole log on disk, not the preview. A finished foreground call is read by its tool call id, through `<session>/terminal/<id>.log`, even though the terminal no longer tracks it. An id that is not a plain file name is refused.
