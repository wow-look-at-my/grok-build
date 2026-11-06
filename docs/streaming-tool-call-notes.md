# Streaming tool-call notes

- A call's arguments reach the pager as the model writes them. The path is `SamplingEvent::ToolCallDelta` → `XaiSessionUpdate::ToolCallDeltaChunk` → `AcpUpdateTracker::handle_tool_call_delta`. The real `ToolCall` then adopts the row those deltas built. So a call keeps the position it held while it was typed.
- The row shows the CONTENT. It does not show a progress number alone. `OtherToolCallBlock::streaming_preview` holds the decoded tail of the arguments. The block draws it under the header in every display mode. `Collapsed` is included, because that is the mode a call is in while it streams. A preview drawn only when expanded left a write that read `◆ write` and nothing else until the file was whole.
- `StreamingArgsTail` (`acp/streaming_args.rs`) decodes the JSON escapes as the fragments land. A provider splits the arguments at arbitrary offsets. So an escape sequence can straddle a fragment boundary. The decoder carries that state across. A raw fragment puts `\n` on the screen and draws a file write as one line thousands of columns wide.
- The tail is bounded in both directions. It drops the older lines. One line stops growing at its character cap. A tab becomes spaces. So a large write costs a fixed row height however long it runs.
- The byte count beside the name is what the tail cannot say. A tail of a large body looks the same at any size.
- The preview truncates each line. It never wraps one. Every fragment redraws it. A wrapped line changes the block's height as the model types, which makes the whole transcript jump.
