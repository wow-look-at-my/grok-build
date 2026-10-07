# Verdict: cost-attribution order-independence claim

Status: VERIFIED (true with caveats) for the shipped `set_last_turn_cost`, with
residual eviction risk reduced by a code change.

## Claim
"Attribution logic is order-independent but relies on prompt IDs being present
and stable; when no prompt key is available it falls back to the
most-recently-finished entry, which is unambiguous only when a single turn is
in flight. The bounded prompt→entry map (max 8 entries) means a very late
TurnCompleted beyond that window would not attach its cost; this is an
acceptable trade-off to bound memory."

## Analysis (grounded in shipped code)

### 1. Order-independence: TRUE for the keyed path
`AcpUpdateTracker::set_last_turn_cost` (src/acp/tracker.rs) resolves a reported
cost via:
- branch (1) the still-streaming `current_agent_msg`, guarded by
  `streaming_matches` over `(prompt, current_prompt_id)`
  (`(None,_)=true; (Some(p),Some(cur))=p==cur; (Some,_)=true`);
- branch (2) the prompt→entry map `finished_prompt_costs` recorded by
  `finish_turn`;
- branch (3) the keyless fallback `last_finished_agent_entry`.

For a KEYED notification the order of `TurnCompleted` vs. `finish_turn` does
not matter: before the driver's `PromptResponse` finish it reaches the same
streaming entry via branch (1) (prompt == running turn); after finish it reaches
the same finished entry via branch (2) (prompt in map). So the keyed path is
order-independent — CONTINGENT on prompt IDs being present and stable.

Key caveat: the shipped call site always passes `Some(&prompt_id)` from the wire
(previously session_notification.rs:347), so the keyless path is defensively
dead today. If a key is ever absent, order-independence degrades.

### 2. Keyless fallback (branch 3) ambiguity
Branch (3) is reached only when branches (1) and (2) miss. Branch (1) requires a
streaming block to attach; so in the idle state (nothing streaming) with a map
miss, branch (3) is the only path. `last_finished_agent_entry` is cleared
whenever a new agent message starts streaming (`handle_agent_chunk`), so branch
(3) is unambiguous iff exactly one turn has finished since the last clear; with
multiple finished turns its attribution to the older turn is wrong.

### 3. Eviction: the claim UNDERSTATES it (corrected)
A prompt-keyed notification whose key is older than the 8-entry bounded map
(evicted via `len() > 8 → remove(0)` in `finish_turn`):
- during a newer stream → dropped (branch 1 prompt-mismatch rejects, branch 3
  sees `last_finished_agent_entry` already cleared);
- IDLE → previously mis-attributed to the most-recently-finished entry via
  branch (3) — an ACTIVE corruption, not a benign "would not attach its cost."

The claim's "acceptable trade-off" phrasing understates this idle-time
mis-attribution.

## Risk reduction implemented
- Guard: branch (3) now fires only for genuinely keyless inputs
  (`prompt.is_none()`). A prompt-keyed notification that misses both the
  streaming block and the map is DROPPED (safe absence), never falling through
  to `last_finished_agent_entry`. This converts eviction-triggered
  mis-attribution into a safe absence.
- Prompt-ID reuse treated as an explicit assumption (map dedups newest-wins).
- 8-entry map bound documented as the memory-vs-latency trade-off.

## Tests (direct, on shipped `set_last_turn_cost`, no re-implementation)
In `acp::tracker::tests`:
- set_last_turn_cost_keyed_evicted_idle_is_dropped_not_misattributed
- set_last_turn_cost_keyless_finished_attaches_to_last_finished
- set_last_turn_cost_keyless_with_streaming_attaches_to_stream
- set_last_turn_cost_keyed_streaming_matches_running_prompt
- set_last_turn_cost_keyed_streaming_mismatch_skips_stream

Green: set_last_turn_cost 5/5, acp::tracker 158/158, acp_handler 428/428,
turn_completion 58/58.
