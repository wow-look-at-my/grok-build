use std::future::Future;
use tokio::task::JoinHandle;
use tracing::{Instrument, Span};

/// Utility macro for propagating the current tracing context to a newly spawned task.
///
/// Note: The spawned task will be associated with the currently active span. To create a *new*
/// span for the spawned task, manually instrument the future using [tracing::Instrument] instead of
/// using this macro. For example:
///
/// use tracing::{info_span, Instrument};
///
/// let fut = tokio::spawn(async move {
///     print!("do stuff")
/// }.instrument(info_span!("spawned task")));
#[allow(clippy::disallowed_methods)] // the instrumenting wrapper itself
pub fn spawn_traced<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    // This is the instrumenting wrapper itself: it returns the `JoinHandle` to
    // its caller rather than dropping it, so the task's failure is the caller's
    // to await. Banning the call here would only hide the one spawn every
    // traced task in the workspace goes through.
    tokio::spawn(future.instrument(Span::current()))
}
