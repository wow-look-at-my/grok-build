# Memory notes

Memory-v2 needs no command. It has none for saving or consolidating. There is no `/flush` and no `/dream`: the shell's builtin table and the pager's registry hold neither. The `memory/flush` and `memory/dream` extension methods stay for an ACP client that wants to wait for a capture or force a Dream.

## Capture

- A capture job runs after every completed or stopped prompt turn (`enqueue_v2_turn_capture` in `acp_session_impl/memory_capture.rs`). The extractor has no tools. Its only write is the host's `V2CaptureStore::commit`.
- `EXTRACTION_SYSTEM_PROMPT` names what to keep, because the user never asks for a memory: where things are, user preferences, corrections, project facts. It refuses a limit that the user set for one request ("do not run it now"), because a later session reads a stored limit as a standing rule. The refusal covers a limit written as a clause of another fact. A change to it needs a new `PROMPT_VERSION` (`session/memory/v2_capture.rs`).
- Each writer of memory carries the refusal: capture, the Dream prompt (`V2_DREAM_SYSTEM_PROMPT`), and the main agent. The `<memory>` section of `xai-grok-agent/templates/prompt.md` lets the agent write a topic itself with its ordinary write tool, only when the user asks. That write skips capture and Dream, so their rules never see it. An agent told to write any useful fact stored the limit in a topic in a few of the live runs.
- The request carries `recorded_memory_index`, the manifests of both scopes. The extractor skips a fact that the index lists.
- Each observation has a `scope`. `parse_scoped_model_outcome` splits them. A `global` one holds in every repository and goes to the global scope. An absent scope is the workspace.
- `commit_global_observations` runs BEFORE the workspace commit, under the range of the workspace job. A crash between the two leaves the workspace job to run again. The finished global job then takes nothing more. The other order loses the global observations.

## Consolidation (Dream)

- `V2_DREAM_SCOPES` lists both scopes. Promotion, garbage collection and Dream loop over it. A new scope-bound step must use it, or the global inbox grows without limit.
- `evaluate_automatic_v2_dream` runs after a capture that wrote observations and at session start (`resume_v2_capture`). Nothing polls it. The gate is `V2_DREAM_MIN_PENDING_COUNT` or `V2_DREAM_MAX_PENDING_AGE`.
- A forced Dream (`memory/dream`) runs both scopes. `combine_scope_outcomes` reports the first failure, and a scope with no work never hides the other.

## Recall

- `inject_memory_recall` (`acp_session_impl/memory_recall.rs`) is not a tool. It runs before each model round on the conversation items it has not read. A new prompt (`recall_prompt`: a person's message, an interjection or another agent's message) is the whole query.
- A round with no new prompt queries the model's own new prose (`recall_model_text`), with `V2RecallQuery::mid_task` set. Tool-call arguments and tool results stay out: they hold paths and code that share words with every note. Mid-task gates are stricter. A word match needs matched terms that name the note's subject and one more matched term. A semantic hit needs `MID_TASK_EXTRA_STANDARD_SCORE` more, and at most one note comes back. Without those gates a note matched on almost every round.
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
- With no `[memory.embedding].model`, recall takes an embedding model from a configured local runtime (`SessionRecall::active_embedding`). The Ollama and LM Studio listings record their embedding models (`record_embedding_models` in `remote/local_runtime.rs`) and leave them out of the chat catalog. The lookup runs at each recall, so a session that started before the listing still gets the model. No listing is made for recall: a runtime that is no configured provider is never asked.
- The memory listing carries `recall_embedding`, the model in use. The memory browser draws it. An automatic choice is on screen.
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
- The memory browser draws `recall_status_line` above the cost line: word matching alone, or the embedding model with its endpoint. It reads the options the pager started with (`AppView::memory_recall_line`), because the agent runs with those. A row edited since then does not change it.

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
- `xai-grok-pager-pty-harness`: `live_model::live_later_session_knows_what_an_earlier_session_was_told_in_passing` runs the real pager through sessions in separate processes. The earlier one hears a preference and a location in passing. The later one must answer with the location and apply the preference. Only memory carries them over.
- `xai-grok-pager-pty-harness`: `pty_e2e/memory_recall_uses_a_local_embedding_model.rs` runs the pager with a mock Ollama as a provider and no embedding config. A prompt that shares no word with a topic must recall it, and `/memory` must name the model.
- `xai-grok-pager-pty-harness`: `live_model::live_recall_matches_by_meaning_through_a_real_ollama` runs that same session against a real Ollama. It needs `GROK_LIVE_OLLAMA_URL` (the `/v1` base URL) and `GROK_LIVE_OLLAMA_EMBEDDING_MODEL` (the embedding model the runtime lists first). Run it when the listing code changes: the mock only repeats what the code expects of `/api/show`.
- `xai-grok-pager-pty-harness`: `live_model::live_a_note_is_recalled_again_after_a_compaction` runs the real pager against a live model. A note is recalled for the first prompt about it, not for the second, and again for the first prompt after `/compact`.
- `xai-grok-shell`: `tests/it/memory_recall_e2e.rs` drives a real agent against the mock model. It needs a workspace outside the system temp directory, because memory is off for a temp workspace.
- `xai-grok-pager-pty-harness`: `pty_e2e/memory_cost_is_visible.rs` runs the pager against a mock that reports a price. It asserts the `mem` figure in the status bar and the cost line in `/memory`.
- `xai-grok-pager-pty-harness`: `pty_e2e/memory_settings_pty.rs` edits the Memory rows in the real pager. It reads `config.json` after each edit and opens the rows again after a restart.
- `MvpAgent::new` does not read `Config::memory_config`. The binary's startup calls `set_memory_config` after it. The test harness does the same only through `connect_and_auth_with_memory`, so every other harness session runs with no memory.
