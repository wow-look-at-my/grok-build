# Output-budget notes

- A provider charges the requested output against the same window as the prompt. A conversation that is under the window on its own can still put the REQUEST over it. 737_857 input tokens plus a 262_144 output budget is 1_000_001 against a 1M window. The server answers 400. `max_output_tokens` is not a free parameter of the request. It is whatever the window has left.
- `xai_token_estimation::fit_output_tokens` is that arithmetic. `ConversationRequest::fit_output_budget` applies it. `build_conversation_request` (chat-state) fits against the tracked total. That total is the provider's own usage for the last response plus the estimated delta, which is the best number this process has.
- `apply_conversation_defaults` (sampler) fits again, against its own bytes/4 estimate. The sampler's DEFAULT budget is what a caller that sets none sends. Every backend converter reads the field from there. This pass only cuts further.
- Both fit against `window_less_estimate_slack`, which holds back 1% of the window. Every prompt count here is an estimate somewhere. One token low is a rejected request.
- The budget never goes below `MIN_OUTPUT_TOKENS`. A prompt that leaves less room than that is over the window. The provider's own overflow error is the honest report of it. The compaction ladder answers that error.
- `check_preflight_overflow` and `should_compact_on_error` measure against the window less that same floor. A prompt with no room for an answer needs compaction, not a 1024-token reply.
- `should_compact_on_error` also takes the server's own context-length message as decisive. Its tokenizer is the one that counts. A rejection that names the context length is never a turn to hand back to the user.
