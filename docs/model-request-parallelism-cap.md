# Model-request parallelism cap

- `ui.max_parallel_requests` (default 7, `0` = no cap) is the most model requests one process sends at once (`xai-grok-sampler/src/request_slots.rs`). A request past it waits in a FIFO queue. Every config load applies it (`apply_max_parallel_requests` in `util/config/campaigns.rs`), so a reload changes it live. A lowered cap never cuts off a request in flight.
- The slot is taken in the client's send methods and held until the response stream is dropped. So every caller is covered: the sampler actor, `conversation_collect`, compaction's direct stream calls, and `web_search`.
- The same send methods meter the request for billing (`usage_sink.rs`). The actor's held slot skips the second slot, not the meter, so each try is billed once. See `cost-indicator-feature-notes.md`.
- The actor takes its slot in `run_one_attempt` BEFORE `FirstTokenDeadline::start`, and runs the send under `with_slot_held` so the client takes no second one. The TTFT limit, the idle timeout and the rate gate therefore all start after the queue.
- A caller's own deadline around a model call uses `timeout_excluding_queue`, not `tokio::time::timeout`. It moves the deadline by the time spent queued. The clock is a task-local, and `Submit` carries it to the actor's task by hand.
- A rate-floor backup takes its own slot. With every slot in use it waits, and the slow original keeps streaming.
- A queued try says so: `SamplingEvent::Queued`/`Dequeued` → transient `XaiSessionUpdate::RequestQueued`/`RequestDequeued` → `AcpUpdateTracker::request_queued`. The status row then reads `Queued: 7 model requests already running…` (`WaitingReason::Queued`) in place of "Waiting for response". Without it a queued turn looks like a stalled model.
