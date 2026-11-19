# Memory notes

Memory-v2 needs no command. It has none for saving or consolidating. There is no `/flush` and no `/dream`: the shell's builtin table and the pager's registry hold neither. The `memory/flush` and `memory/dream` extension methods stay for an ACP client that wants to wait for a capture or force a Dream.

## Capture

- A capture job runs after every completed or stopped prompt turn (`enqueue_v2_turn_capture` in `acp_session_impl/memory_capture.rs`). The extractor has no tools. Its only write is the host's `V2CaptureStore::commit`.
- `EXTRACTION_SYSTEM_PROMPT` names what to keep, because the user never asks for a memory: where things are, user preferences, corrections, project facts. A change to it needs a new `PROMPT_VERSION` (`session/memory/v2_capture.rs`).
- The request carries `recorded_memory_index`, the manifests of both scopes. The extractor skips a fact that the index lists.
- Each observation has a `scope`. `parse_scoped_model_outcome` splits them. A `global` one holds in every repository and goes to the global scope. An absent scope is the workspace.
- `commit_global_observations` runs BEFORE the workspace commit, under the range of the workspace job. A crash between the two leaves the workspace job to run again. The finished global job then takes nothing more. The other order loses the global observations.

## Consolidation (Dream)

- `V2_DREAM_SCOPES` lists both scopes. Promotion, garbage collection and Dream loop over it. A new scope-bound step must use it, or the global inbox grows without limit.
- `evaluate_automatic_v2_dream` runs after a capture that wrote observations and at session start (`resume_v2_capture`). Nothing polls it. The gate is `V2_DREAM_MIN_PENDING_COUNT` or `V2_DREAM_MAX_PENDING_AGE`.
- A forced Dream (`memory/dream`) runs both scopes. `combine_scope_outcomes` reports the first failure, and a scope with no work never hides the other.

## Recall

- `inject_memory_recall` (`acp_session_impl/memory_recall.rs`) is not a tool. It searches only when the conversation holds a prompt it has not read (`recall_prompt`): a person's message, an interjection or another agent's message. A model round with no new prompt searches nothing.
- The query is that newest prompt and nothing else. The model's own text and its tool-call arguments stay out: they share words with every note of the same repository. With them in the query, a note matches on almost every round.
- An observation that the asking session captured is never returned (`V2RecallQuery::session_id`). Capture writes it from the conversation, so a reminder of it repeats what was just said.
- A file written since the live conversation began is never returned (`V2RecallQuery::conversation_start`, kept in `RecallState`). Consolidation turns the session's observations into a topic within minutes, and a topic names no session. A compaction clears the start time, because the conversation no longer holds what those files say.
- A subagent runs no recall. Its prompt is a brief its parent wrote.
- A file is returned one time for a conversation, one chunk for each recall. The reminder names the file, so its other chunks need no reminder. A recall returns at most `max_hits` files.
- A hit goes to the model as a `<memory-recall>` reminder and to the client as `MemoryRecalled`. Each line carries `ref <key>`. `seen_recall_keys` reads those keys back from the live conversation. That is the whole "one time for each conversation" rule. A compaction, a rewind, a fork or a resume needs no extra state.
- `RecallState::reset` runs where `context_injected` is cleared. A conversation shorter than the cursor also resets it.
- The engine is `xai_grok_memory::V2Recall` (`v2_recall.rs`). It reads `topics/` and `observations/_inbox/` of both scopes into memory and reads a file again only when its size or modification time changes. It never reads a path that `excluded_manifest_paths` hides.
- Word matching needs no database. A chunk qualifies on the summed inverse document frequency of the query terms it holds, so a word that every chunk has adds nothing. Observation bookkeeping keys (session id, model name) are not searchable text.
- A word match must name the chunk's subject. One matched term sits in the file title, a heading of the chunk, or an observation's keywords and aliases (`subject_terms`). Terms that only the body shares say the prompt and the note use the same vocabulary. They do not say the prompt is about the note.
- Only the most distinctive query terms are matched (`max_query_terms`): frequent in the query, rare in the corpus.
- A standard score judges a semantic hit, not by a fixed cosine. Each embedding model puts cosines on its own scale: one model scores every pair high, another scores a true match low. `BASELINE_NOTES` are embedded once for each model. A chunk's cosine with the query is compared with the mean and the spread of the query's cosines with those notes. `min_standard_score` is the gate. `runner_up_fraction` drops a weaker chunk that sits beside a strong one.
- Embedding calls go to the chat model's endpoint unless `[memory.embedding].base_url` names another one (`embedding_endpoint` in `spawn.rs`). Its key comes from the variable `api_key_env` names. The chat model's key never goes there. An Anthropic-style provider serves no `/embeddings` route. It needs this.
- A request to that separate endpoint carries `dimensions` only when the config sets it (`request_dimensions`). Ollama refuses a size above the model's own. The default size is not the model's.
- Missing texts go out in requests of `embed_batch_size`, the baseline first, and each request stores its vectors. A failed or late request keeps the ones before it. No new request starts after `embed_time_budget`. No chunk is judged while a baseline note has no vector.
- `EmbeddingFailures` (`memory_state.rs`) decides when `MemoryRecallDegraded` goes out, one time for a session. After failed recalls in a row, or at once for a provider with no credential. A local model misses one deadline while it loads. The pager prints the notice. Recall keeps its word matches.
- Vectors are optional. A cached vector is deleted when its chunk leaves the corpus. With `[memory.embedding].model` set, chunk vectors are cached in the `recall_embeddings` table of the scope's `index.sqlite`, keyed by text hash and model. A provider failure or a slow call returns the word matches and logs one error for the session. It never fails the turn.
- `[memory_v2] recall_enabled = false` turns recall off.

## Cost

- Every memory model call is billed to the session ledger like any side call. Its client also carries a `MemoryUsageSink` (`session/billed_client.rs`), which adds the price of the call to one total in `MemoryV2TokenTotals`: capture, consolidation or recall. `memory_billed_client` wraps a chat client. Recall wraps the sink of its embedding provider.
- The totals come from the sink, not from a response. A retry, a backend switch and a call that failed after its usage arrived are all in them. `price_model_call` prices a call whose provider reports no cost.
- `report_memory_cost` sends `MemoryCost` with the totals and the new session total after each capture, each Dream pass and each recall that used a provider. An unchanged total sends nothing. The pager adopts the session total and shows the memory share in the status bar and in the `/memory` modal.
- A new kind of memory model call must go through `memory_billed_client` or `memory_usage_sink`. Otherwise it is billed but missing from the memory share.

## Settings

- `/settings` → Memory is a group row (`MEMORY_CHILDREN` in the pager's `settings/defs.rs`). Its rows edit `memory.v2.recall_enabled` and the `memory.embedding` keys.
- One action carries every row: `Action::SetMemorySetting { key, value }`. The shell's `MemorySetting` maps a key to its internal table path, and `apply_memory_setting` writes it. An empty text or a size of 0 removes the key.
- The agent reads the memory config at startup (`set_memory_config`). A change needs a restart. The toast says so.

## Tests

- `xai-grok-memory`: `v2_recall_tests.rs` drives the engine on real scope directories.
- `xai-grok-memory`: `v2_recall_live_tests.rs` runs recall against a served embedding model. Its tests are ignored, because CI serves no model. The prompts share no distinctive word with their note, and word matching is off, so each hit comes from the model alone. Run it when the gate or the query changes:

```bash
GROK_LIVE_EMBEDDING_BASE_URL=http://127.0.0.1:8791/v1 GROK_LIVE_EMBEDDING_MODEL=bge-small-en-v1.5 \
  cargo test -p xai-grok-memory --lib live_ -- --ignored --nocapture
```

```text
model                  paraphrases recalled   wrong note first   unrelated prompts that recalled a note
bge-small-en-v1.5      4 of 9                 0                  0 of 12
all-MiniLM-L6-v2       5 of 9                 0                  0 of 12
gte-small              4 of 9                 0                  0 of 12
nomic-embed-text-v1.5  5 of 9                 0                  1 of 12
bge-base-en-v1.5       4 of 9                 0                  0 of 12
bge-large-en-v1.5      7 of 9                 0                  0 of 12
bge-m3                 3 of 9                 0                  0 of 12
e5-large-v2            3 of 9                 0                  0 of 12
multilingual-e5-large  3 of 9                 0                  0 of 12
mxbai-embed-large-v1   7 of 9                 0                  2 of 12
all-mpnet-base-v2      7 of 9                 0                  2 of 12
```

On the last models a true match and a false one score the same (3.7 against 3.6), so no gate separates them. A false recall costs one reminder that the model can ignore.
- `xai-grok-pager-pty-harness`: `live_model::live_session_recalls_a_note_only_for_the_prompt_that_asks_about_it` runs the real pager through a session of several prompts against a live model, with tools and capture on. It prints the notes recalled for each prompt. Run it when a recall rule changes. It needs the `GROK_LIVE_*` variables that [background-waits-and-the-goal-check.md](background-waits-and-the-goal-check.md) names.
- `xai-grok-shell`: `tests/it/memory_recall_e2e.rs` drives a real agent against the mock model. It needs a workspace outside the system temp directory, because memory is off for a temp workspace.
- `xai-grok-pager-pty-harness`: `pty_e2e/memory_cost_is_visible.rs` runs the pager against a mock that reports a price. It asserts the `mem` figure in the status bar and the cost line in `/memory`.
- `xai-grok-pager-pty-harness`: `pty_e2e/memory_settings_pty.rs` edits the Memory rows in the real pager. It reads `config.json` after each edit and opens the rows again after a restart.
- `MvpAgent::new` does not read `Config::memory_config`. The binary's startup calls `set_memory_config` after it. The test harness does the same only through `connect_and_auth_with_memory`, so every other harness session runs with no memory.
