# Cross-Session Memory

Memory lets Grok recall facts, decisions, and patterns from earlier sessions. Grok indexes the information you save and searches it automatically, so a new session can reuse relevant context.

---

## What Is Memory?

Without memory, each Grok session starts fresh: the model knows nothing about previous sessions. When you enable memory, Grok can:

- Recall project conventions you explained before.
- Reuse debugging steps that worked.
- Carry architectural decisions forward across sessions.
- Avoid re-asking questions it already has answers to.

Memory is on by default and uses the v2 topic and observation pipeline.

### How memory is organized

Memory has two scopes. Global memory holds facts that apply across all your
projects; workspace memory holds facts about one repository. Clones and
worktrees of the same repository share one workspace scope.

Each scope keeps its knowledge as ordinary Markdown files. `topics/` holds curated notes, one file per subject, and is what Grok reads at the start of a session. New facts captured after each turn, including a turn you stop before it finishes, land as small observations. Those observations are ones that a later consolidation pass folds into topics. A bounded generated index of both scopes is injected into the model's context once per session so it can decide which topics to open.

### Memory runs by itself

You do not tell Grok to remember. You do not run a command to save.

- **Capture.** After every turn, a background call reads the turn and records what a later session needs. Where things are (files, commands, URLs, config keys), your preferences, your corrections, and project facts with their reasons. It does not record a fact the memory index already lists.
- **Scope.** A fact that holds in every repository, such as how you like to work, goes to global memory and follows you to other projects. A fact about this repository goes to workspace memory.
- **Consolidation.** Observations become topics when enough of them wait, or when the oldest one waited a while. Grok checks this after each capture and when a session starts, so notes from yesterday are topics today.
- **Recall.** As the conversation moves, Grok searches stored memory with the new text: your prompt, its own replies and the arguments of its tool calls. A note that matches is shown to the model with its file path, and the scrollback shows a `Recalled from memory` line. Each note is shown one time for each conversation. A compaction removes the reminders. A note can be shown again after it.

Recall matches on words by default. Set `memory.embedding.model` to an embedding model your provider serves to add meaning-based matches. Recall then also finds a note that shares no word with what you typed. It compares your prompt, not the whole conversation, with each note, and it adjusts to the model you chose, so no similarity threshold needs tuning. If the embedding call fails, recall keeps the word matches and logs the error.

There is no command for saving or consolidating: both run by themselves. To turn recall off, set `memory.v2.recall_enabled` to `false`.

### What memory costs

Capture, consolidation and recall each call a model, and each call is billed to the session. These calls are part of the session total in the status bar. When memory spent anything, the status bar adds the memory share after the total, for example `$0.52 · mem $0.04`. The `/memory` modal shows the same share split into capture, consolidation and recall.

Notes you recorded with earlier versions of Grok Build are carried over
automatically the first time a workspace is opened after updating: each section
of the earlier notes becomes a topic, and sections whose name already matches a
topic are appended to it under a "From earlier sessions" heading. The earlier
files are left in place unchanged.

Memory product telemetry contains only fixed enums, booleans, counts, and
durations. It never includes prompts, statements, topic names, keywords,
paths, model output, or free-form errors.

---

## Turning Memory Off

Memory needs no setup. To turn it off in every session:

```json
{
  "memory": {
    "enabled": false
  }
}
```

To use the pipeline instead of v2:

```json
{
  "memory": {
    "enabled": true,
    "v2": {
      "enabled": false
    }
  }
}
```

### Force-Disable

To disable memory for the process even when config or remote settings enable it:

```bash
export GROK_MEMORY=0
```

### Mid-Session Toggle

Toggle memory on or off during a session without restarting: open `/memory`
and press `t`.

The toggle is session-scoped -- it does not persist to `config.json`. It works in both directions. A session that started with `memory.enabled` set to `true` can turn memory off. A session that started with `memory.enabled` set to `false` can turn it on. New sessions follow `config.json` again. Toggling off removes access to memory tools and the memory instructions in the system prompt but keeps existing files on disk.

The toggle cannot override the process-wide force-disable (`--no-memory` or `GROK_MEMORY=0`); those hide `/memory` for the whole session.

### Priority Order

1. A process-wide force-disable (`--no-memory` compatibility flag or
   `GROK_MEMORY=0`) turns memory off.
2. An explicit `memory.enabled` set to `false` in the effective config turns memory off, including anything enabled by managed remote settings. The `/memory` `t` toggle can still turn it on for the current session.
3. Otherwise memory is on, in v2 mode. `memory.v2.enabled` set to `false` selects the legacy pipeline, which then needs `GROK_MEMORY=1`, `memory.enabled` set to `true`, or a managed remote setting.

Staged-rollout and kill-switch controls for operators are documented in the
internal hardening notes, not here.

---

## How Memory Is Stored

With `memory.v2.enabled` set to `false` and legacy memory on, files live under `~/.grok/memory/` (`MEMORY.md` plus hashed workspace directories).

---

## Automatic Saves

When a session ends, Grok saves a structured metadata summary to that session's daily log. The summary contains:

- Message counts (user, assistant, and tool results).
- Topics: the first few substantive user prompts from the session, up to five.
- The session date and time (UTC).

Grok builds the summary from conversation metadata without an LLM call, without added latency. Grok skips the save for trivial sessions -- those with fewer than three substantive prompts, or fewer than 50 bytes of user text.

The summary does not record tool usage, file paths, or shell commands. The session ID forms part of the log filename. To turn automatic saves off, set `session.save_on_end = false`. Decisions, patterns, and reasoning are captured after every turn, with no command.

---

## Working with Memory

### Remember

Ask Grok to remember something, and it appends the note to a `MEMORY.md` file -- the workspace file for project-specific items, or the global `~/.grok/memory/MEMORY.md` for cross-project preferences:

```
> remember to always open PR links after pushing
```

Grok records entries as durable statements under organized headings, such as `## Preferences`, `## Project Context`, or `## Debugging`. The file watcher reindexes the change on the next memory search, so the new entry is searchable within the current session.

You can also save a note directly with the `/remember` command:

```
/remember always open PR links after pushing
```

Run `/remember` with no text to enter remember mode, where the next line you type becomes the note. Either way, Grok opens a review panel showing the note (with an optional rewritten version you can toggle with `Tab`); the note is written only after you confirm. On save, Grok shows `Memory saved to ~/.grok/memory/MEMORY.md`.

### Forget

Ask Grok to forget something, and it finds and removes the matching entry:

```
> forget the snake_case convention
```

Forget is best-effort: the model searches memory and removes entries that match. For guaranteed removal, edit the files under `~/.grok/memory/` directly and delete the entry yourself. To locate a file, open the `/memory` browser and press `y` to copy its path.

### Recall

Ask what Grok remembers:

```
> what do you remember?
```

Grok searches across all memory files and summarizes what it knows, grouped by source: global preferences, project-specific knowledge, and session history. Use `/memory` to browse the raw files.

### Direct Editing

You can edit memory files directly under `~/.grok/memory/`. The file watcher reindexes your changes on the next memory search.

---

## Browsing Memory with /memory

The `/memory` command opens a modal showing all memory files:

```
/memory
```

Files are grouped by scope:
- **Global** -- cross-project memory (`MEMORY.md`).
- **Workspace** -- project-specific memory (`MEMORY.md`).
- **Sessions** -- per-session summaries, in reverse chronological order.

The modal uses a split-pane layout: the file list on the left, a read-only content preview on the right. The preview updates as you move through the list.

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `↑`/`↓` or `j`/`k` | Move through the file list |
| `PgUp`/`PgDn` | Jump 10 entries |
| `/` | Filter the file list |
| `Enter` | Read the selected note: the preview takes keyboard focus (arrows, `PgUp`/`PgDn`, `Home`/`End` scroll it) |
| `y` | Copy the selected file's path to the clipboard |
| `x` | Delete the selected note (press `x` again to confirm) |
| `t` | Toggle memory on or off |
| `Ctrl+F` | Toggle fullscreen |
| `Esc` | Close the modal, or leave filter or preview focus |

The filter matches note names and note contents; separate words all have to match. When you filter, the preview scrolls to the first match. If nothing matches, the list says so; `Backspace` clears the filter.

The preview pane is read-only. Scroll it with the mouse wheel, by dragging its scrollbar, or with the keyboard after `Enter`. Drag across the preview text to copy that text to the clipboard; a brief message under the file list confirms every copy. Generated `MEMORY.md` indexes cannot be deleted.

When the memory modal's content area is under 64 columns, the modal shows the file list only and hides the size column; press `Enter` to read the selected note full-width and `Esc` to return to the list.

You can also open `/memory` from the command palette.

---

## Memory Notifications

When you save a note with `/remember`, Grok confirms in the scrollback:

```
Memory saved to ~/.grok/memory/MEMORY.md
```

Background saves (capture, consolidation, and session-end) run silently. A failed consolidation posts one scrollback line. The captured notes stay pending, and consolidation runs again after the next capture or at the next session start. Use `/memory` at any time to browse what Grok stored.

---

## Consolidation

Consolidation (Dream) folds captured notes into a deduplicated set of topics, which reduces noise and improves search quality over time. It runs by itself. There is no command for it.

### Auto-Dream

By default, Grok checks the consolidation gates at launch and periodically during a session. It runs Dream once enough time passes and enough sessions collect:

```json
{
  "memory": {
    "dream": {
      "check_interval_secs": 3600,
      "enabled": true,
      "min_hours": 24,
      "min_sessions": 5
    }
  }
}
```

---

## How Memory Affects Prompts

### First-Turn Injection

On the first turn of each session, Grok automatically searches memory for content relevant to the current project and injects it as context. This means Grok starts with knowledge from previous sessions without a reminder.

First-turn injection can be configured:

```json
{
  "memory": {
    "initial_injection": {
      "enabled": true,
      "min_score": 0.9
    }
  }
}
```

### After Compaction

Memory is also searched after auto-compaction to recover relevant context that may have been discarded.

---

## Memory Search

Grok searches memory automatically, but you can also trigger searches manually in the chat:

```
Search memory for "auth middleware patterns"
Read my workspace MEMORY.md
```

The model has access to two memory tools:
- `memory_search` -- Search across all memory
- `memory_get` -- Read a specific memory file by path

### Search Scoring

The default embedding model is unset, so memory starts in full-text-only mode. If you configure an embedding model, search combines vector similarity (weight `0.7`) with BM25 text similarity (weight `0.3`). Results are filtered by a minimum score threshold (default: `0.7`).

### Source Weights

Each memory source has a weight multiplier applied to its score. All sources default to `1.0`. You can adjust any of them under `memory.search.source_weights`:

| Source | Weight | Description |
|--------|--------|-------------|
| `workspace` | 1.0 | Project-specific memory |
| `session` | 1.0 | Session logs |
| `global` | 1.0 | Cross-project memory |

### Temporal Decay

Session memories decay over time so recent sessions are prioritized:

```json
{
  "memory": {
    "search": {
      "temporal_decay": {
        "enabled": true,
        "half_life_days": 30.0
      }
    }
  }
}
```

Only session chunks decay. Global and workspace memories are exempt since they contain curated long-term knowledge.

### MMR (Maximal Marginal Relevance)

MMR re-ranking penalizes redundant results to improve diversity:

```json
{
  "memory": {
    "search": {
      "mmr": {
        "enabled": true,
        "lambda": 0.7
      }
    }
  }
}
```

---

## CLI Commands

The `grok memory` command manages memory from the shell. It has one subcommand, `clear`:

```bash
# Clear workspace memory (MEMORY.md, sessions/, and index.sqlite). This is the default scope.
grok memory clear

# The same scope, stated explicitly
grok memory clear --workspace

# Clear the global MEMORY.md
grok memory clear --global

# Clear both workspace and global memory
grok memory clear --all

# Skip the confirmation prompt (-y is the short form)
grok memory clear --yes
```

To edit memory from the shell, open the files in your editor directly -- for example, `$EDITOR ~/.grok/memory/MEMORY.md`.

---

## Configuration Reference

### Core Settings (`memory`)

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | on (v2) | Memory switch. `false` turns memory off |
| `session.save_on_end` | `true` | Write metadata summary on session end |
| `watcher.enabled` | `true` | Watch `~/.grok/memory/` for external edits and reindex |

### Index Settings (`memory.index`)

| Key | Default | Description |
|-----|---------|-------------|
| `max_chunk_chars` | `1600` | Maximum chunk size in characters |
| `chunk_overlap_chars` | `320` | Character overlap between chunks |

### Embedding Settings (`memory.embedding`)

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `"api"` | Embedding provider (currently `"api"`) |
| `model` | unset | Embedding model name. Unset or `""` uses full-text-only retrieval. |
| `dimensions` | `1024` | Embedding vector dimensions |
| `base_url` | unset | OpenAI-style base URL for embedding calls, for example a local runtime. Unset sends them to the chat model's endpoint. |
| `api_key_env` | unset | Name of the environment variable that holds the key for `base_url`. Leave it unset for an endpoint that takes no key. |

A chat provider that speaks the Anthropic Messages API serves no embedding route. Point `base_url` at an endpoint that does:

```toml
[memory.embedding]
model = "nomic-embed-text"
base_url = "http://127.0.0.1:11434/v1"
```

The chat model's key is never sent to `base_url`. When the embedding lookup cannot run, the session says so one time on screen and recall keeps matching on words.

### Search Settings (`memory.search`)

| Key | Default | Description |
|-----|---------|-------------|
| `max_results` | `6` | Maximum search results |
| `min_score` | `0.7` | Minimum relevance score |
| `vector_weight` | `0.7` | Weight for vector similarity |
| `text_weight` | `0.3` | Weight for BM25 text similarity |

### Initial Injection Settings (`memory.initial_injection`)

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `true` | Enable first-turn memory injection |
| `min_score` | `0.9` | Score threshold for first-turn results |

### Dream Settings (`memory.dream`)

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `true` | Enable automatic Dream consolidation |
| `min_hours` | `24` | Minimum hours between consolidations |
| `min_sessions` | `5` | Minimum sessions since the last consolidation |
| `stale_lock_secs` | `3600` | Seconds before a stale consolidation lock is reclaimed |
| `check_interval_secs` | `3600` | Periodic Dream-gate check interval in seconds. Set `0` to disable periodic checks. |

### Flush Settings (`session.compaction.memory_flush`)

You configure flush under `session.compaction`, not `memory`, because it is a compaction behavior.

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `true` | Enable the pre-compaction memory flush |
| `soft_threshold_tokens` | `4000` | Token headroom before the compact threshold that triggers a flush |
| `max_flush_write_chars` | `8000` | Maximum characters the flush may write to memory |
| `flush_model` | unset | Model for the flush turn. When unset or `""`, Grok uses the session's primary model. |
| `idle_timeout_secs` | `300` | Idle seconds before a background flush. Set `0` to disable idle flushes. |
| `semantic_dedup_threshold` | unset | Cosine-similarity threshold for de-duplicating flushed content. When unset, defaults to `0.92`. |

### Pruning Settings (`session.compaction.pruning`)

You configure pruning under `session.compaction`, not `memory`, because it is a compaction behavior.

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `true` | Enable tool-result pruning |
| `keep_last_n_turns` | `3` | Number of recent turns whose tool results are never pruned |
| `soft_trim_threshold` | `4000` | Character threshold above which old tool results are soft-trimmed |
| `soft_trim_head` | `1500` | Characters kept from the start of a soft-trimmed result |
| `soft_trim_tail` | `1500` | Characters kept from the end of a soft-trimmed result |
| `hard_clear_age_turns` | `10` | Turn age after which tool results are replaced with a placeholder |

---

## Memory Staleness

When a session memory is old, Grok attaches a staleness note to it in search results. Older results get a stronger reminder to verify the current state before you rely on them. These notes help you spot stored facts that might no longer be accurate. Global and workspace memories never receive staleness notes, because they hold curated long-term knowledge.

---

## File Watcher

By default, Grok watches `~/.grok/memory/` for external file changes. If you edit memory files directly (e.g., in your editor), the changes are picked up automatically on the next memory search:

- Created or modified files are reindexed.
- Deleted files have their stale chunks removed from the index.

```json
{
  "memory": {
    "watcher": {
      "enabled": true
    }
  }
}
```

---

## Troubleshooting

### Memory Not Working

1. Verify memory is enabled: check `grok inspect` output.
2. Check `GROK_MEMORY` or `memory.enabled` in the effective config.
3. Check for `GROK_MEMORY=0` or a deprecated compatibility flag overriding config.

### Memory Not Appearing in Sessions

Memory is injected on the first turn. If you started a session before enabling memory, start a new session with `/new`.

### Viewing Memory Files

Use `/memory` in the TUI to browse all memory files with a preview. You can also access them directly:

```bash
ls ~/.grok/memory/
cat ~/.grok/memory/MEMORY.md
$EDITOR ~/.grok/memory/MEMORY.md
```

### Debug Logging

```bash
RUST_LOG=debug GROK_LOG_FILE=/tmp/grok.log grok
grep "memory" /tmp/grok.log
```
