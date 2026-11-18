# Memory notes

Memory-v2 needs no command. `/flush` and `/dream` are manual overrides of work that runs by itself.

## Capture

- A capture job runs after every completed or stopped prompt turn (`enqueue_v2_turn_capture` in `acp_session_impl/memory_capture.rs`). The extractor has no tools. Its only write is the host's `V2CaptureStore::commit`.
- `EXTRACTION_SYSTEM_PROMPT` names what to keep, because the user never asks for a memory: where things are, user preferences, corrections, project facts. A change to it needs a new `PROMPT_VERSION` (`session/memory/v2_capture.rs`).
- The request carries `recorded_memory_index`, the manifests of both scopes. The extractor skips a fact that the index lists.
- Each observation has a `scope`. `parse_scoped_model_outcome` splits them. A `global` one holds in every repository and goes to the global scope. An absent scope is the workspace.
- `commit_global_observations` runs BEFORE the workspace commit, under the range of the workspace job. A crash between the two leaves the workspace job to run again. The finished global job then takes nothing more. The other order loses the global observations.

## Consolidation (Dream)

- `V2_DREAM_SCOPES` lists both scopes. Promotion, garbage collection and Dream loop over it. A new scope-bound step must use it, or the global inbox grows without limit.
- `evaluate_automatic_v2_dream` runs after a capture that wrote observations and at session start (`resume_v2_capture`). Nothing polls it. The gate is `V2_DREAM_MIN_PENDING_COUNT` or `V2_DREAM_MAX_PENDING_AGE`.
- `/dream` runs both scopes. `combine_scope_outcomes` reports the first failure, and a scope with no work never hides the other.

## Recall

- `inject_memory_recall` (`acp_session_impl/memory_recall.rs`) runs before each model round. It is not a tool. The first round of a turn always runs. A later round waits for `MID_TURN_MIN_CHARS` of new text.
- The query is the text added since the last recall: user prompts, assistant text and tool-call arguments. Tool results and injected items stay out. A recall reminder inside the query matches the note it names.
- A hit goes to the model as a `<memory-recall>` reminder and to the client as `MemoryRecalled`. Each line carries `ref <key>`. `seen_recall_keys` reads those keys back from the live conversation. That is the whole "one time for each conversation" rule. A compaction, a rewind, a fork or a resume needs no extra state.
- `RecallState::reset` runs where `context_injected` is cleared. A conversation shorter than the cursor also resets it.
- The engine is `xai_grok_memory::V2Recall` (`v2_recall.rs`). It reads `topics/` and `observations/_inbox/` of both scopes into memory and reads a file again only when its size or modification time changes. It never reads a path that `excluded_manifest_paths` hides.
- Word matching needs no database. A chunk qualifies on the summed inverse document frequency of the query terms it holds, so a word that every chunk has adds nothing. Observation bookkeeping keys (session id, model name) are not searchable text.
- Vectors are optional. With `[memory.embedding].model` set, chunk vectors are cached in the `recall_embeddings` table of the scope's `index.sqlite`, keyed by text hash and model. A provider failure or a slow call returns the word matches and logs one error for the session. It never fails the turn.
- `[memory_v2] recall_enabled = false` turns recall off.

## Tests

- `xai-grok-memory`: `v2_recall_tests.rs` drives the engine on real scope directories.
- `xai-grok-shell`: `tests/it/memory_recall_e2e.rs` drives a real agent against the mock model. It needs a workspace outside the system temp directory, because memory is off for a temp workspace.
