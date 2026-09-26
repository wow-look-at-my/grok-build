# Output-rate backup generations

The output-rate floor (`[ui].min_output_tokens_per_sec`) judges a streaming model call. When the rate stays under the floor for `output_rate_sustained_secs`, the gate breaches.

A breach does not cancel the slow call. The code is `drive_l2` in `crates/codegen/xai-grok-sampler/src/actor/request_task.rs`.

## The race

On a breach, `BackupLauncher::launch` starts a second attempt of the same request. The slow original keeps streaming to the caller. The backup streams into a buffer that the caller does not see.

The event that comes first decides the result:

- The slow call recovers to the floor or above. The backup is cancelled.
- The slow call completes with a usable response. The backup is cancelled.
- The backup gets more output bytes than the slow call. The backup takes over.
- The backup completes. The backup takes over.
- The slow call fails, or completes empty or truncated. The backup takes over.
- The backup fails. The slow call continues, and the gate re-arms.

A takeover sends `SamplingEvent::Retrying` with kind `OutputRateCollapsed` and no wait. The buffered backup events follow, from its `StreamStarted`. Then the rest of the backup streams live. The consumer sees the same event shape as a reissue. So the session needs no change.

Output bytes count text deltas and tool-call argument deltas. A takeover on overtake therefore never shows the user less output than the slow stream had.

Two spans are not judged at all, because the model is generating in one and waiting for the server in the other. A backend-hosted tool call pauses the gate. So does a streamed tool-call fragment that carries no argument bytes, which is how a provider says it is writing the call without showing that work. The reading holds where it was, and the gap leaves the timeline when the arguments land. See "Output-rate floor notes" in `AGENTS.md`.

## Budget

Each backup spends one unit of the rate budget (`output_rate_max_retries`). The transport retry budget is separate. When the budget is spent, a breach keeps the slow response and logs `output_rate_backup_budget_spent`.

A backup does not start a backup of its own. After a takeover, a breach of the backup's floor ends the attempt. The retry loop then reissues the request, while budget remains.

A caller with `retry_only_before_output` gets no backup. A takeover replaces output that the caller already took.

## Log events

All events use the sampling log target.

- `output_rate_backup_started`
- `output_rate_backup_cancelled`
- `output_rate_backup_adopted`
- `output_rate_backup_failed`
- `output_rate_backup_budget_spent`

## What the backup sends, and the prefix cache

The backup sends the request that the slow call started with, byte for byte. It does not include the partial output of the slow call.

This is checked against vLLM source (V1 engine, automatic prefix caching):

- The cache key is a chain of hashes over full 16-token blocks. Sampling parameters are not in the key. A byte-identical prompt matches every full block.
- A block is cached when it is scheduled, not when its request finishes. So the backup reuses the prompt blocks of the slow call while that call still decodes.
- The backup recomputes one block at most. That is the partial last block, or the whole last block when the prompt fills it exactly.
- When the loser is cancelled, its blocks keep their hashes on an LRU free list. The shared prompt blocks stay pinned by the survivor.

The alternative is to send the prompt plus the partial output. vLLM caches generated blocks too. So this can hit. But a hit needs identical token ids. The chat template closes the assistant turn, BPE can merge differently at the cut. And a reasoning parser moves thinking text out of the content. Any of these misses from the first different block. The reliable form sends token ids to `/v1/completions`, which is a different API from the one this client uses.

A backup misses the cache in these cases:

- A load balancer sends it to another replica. The cache is per engine.
- The request sets `cache_salt`, or sets `prompt_logprobs`, which turns off cache reads.
- The LoRA adapter or the multimodal inputs differ.
- The model is a sliding-window or hybrid Mamba model. Hits are coarser, or depend on the window.
