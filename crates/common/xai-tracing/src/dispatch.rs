use tracing::subscriber::NoSubscriber;

/// Returns `true` when a `tracing` dispatcher (subscriber) is active in the
/// current context — either the thread-scoped default.
pub fn dispatcher_active() -> bool {
    tracing::dispatcher::get_default(|dispatch| !dispatch.is::<NoSubscriber>())
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: relies on no test in this binary installing a *global*
    // subscriber.
    #[test]
    fn without_dispatcher_inactive() {
        assert!(!dispatcher_active());
    }

    #[test]
    fn scoped_dispatcher_active() {
        tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            assert!(dispatcher_active());
        });
        assert!(!dispatcher_active());
    }
}
