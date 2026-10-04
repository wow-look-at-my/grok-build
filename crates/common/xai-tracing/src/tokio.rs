use std::future::Future;
use tokio::task::JoinHandle;
use tracing::{Instrument, Span};

/// Utility macro for propagating the current tracing context to a newly spawned task. Note: The spawned task will be associated.
#[allow(clippy::disallowed_methods)] // the instrumenting wrapper itself
pub fn spawn_traced<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    // This is the instrumenting wrapper itself: it returns the `JoinHandle` to its caller rather than dropping it.
    tokio::spawn(future.instrument(Span::current()))
}
