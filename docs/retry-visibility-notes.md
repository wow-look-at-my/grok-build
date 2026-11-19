# Retry-visibility notes

- A retry says what failed and how long the wait is. `SamplingEvent::Retrying` already carried `reason`, which is the error's own `Display`. It now also carries `retry_in_ms`, the backoff the actor is about to sleep.
- Both ride `RetryState::Retrying` to the pager. It renders `Retrying in 27s (1/5):` and drops the countdown once the wait is over. The retried request is then in flight. See `retry_label` in `views/turn_status.rs`.
- The reason goes on the rows under that header, wrapped and never cut (`retry_reason_lines`). `turn_status::desired_height` gives the row count to the layout. The layout gives it every scrollback row above `SCROLLBACK_MIN_ROWS` and no prompt row. Only an error taller than that ends its last row in `…`.
- `retry_in_ms` is `None` for a retry that sleeps nothing. An image strip, a reasoning strip and a message-property strip are those. A shell older than the field also sends `None`.
- The countdown needs no tick source of its own. `tick_demand` already reports Fast for a session whose turn runs, which a retry always is.
- A server `Retry-After` is clamped to `MAX_RETRY_BACKOFF` on the 429 path too, not just the generic one. A per-minute bucket answers `Retry-After: 60`, and one try then sat idle for the whole minute. The limit had frequently cleared sooner.
- `RATE_LIMIT_RETRY_THRESHOLD` covers the wait the server asked for across several attempts. So the turn fails no earlier than before, and an try in between can find the limit clear.
