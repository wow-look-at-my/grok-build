//! Making a detached task's panic reach whoever is waiting on its work.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;

/// Text describing what a panic carried.
///
/// Both payloads `panic!` itself produces are a `&'static str` (a literal) and
/// a `String` (a formatted one). Anything else still has to be reportable, so
/// it is named as a non-message rather than reported as nothing.
pub fn panic_payload(panic: &(dyn Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&'static str>() {
        return (*text).to_string();
    }
    if let Some(text) = panic.downcast_ref::<String>() {
        return text.clone();
    }
    "panicked with a payload that is not a message".to_string()
}

/// Run `task` where a panic is a value rather than a lost task.
///
/// `what` names the work in the log line, so a panic in detached work is
/// attributable without a backtrace. The panic hook still runs; this adds the
/// caller's view of the same failure.
pub async fn guarded<F: Future>(what: &'static str, task: F) -> Result<F::Output, String> {
    match AssertUnwindSafe(task).catch_unwind().await {
        Ok(output) => Ok(output),
        Err(panic) => {
            let detail = panic_payload(&*panic);
            tracing::error!(task = what, panic = %detail, "detached work panicked");
            Err(detail)
        }
    }
}

/// Run work that nobody awaits, so its panic is named instead of lost. A
/// [`guarded`] caller can still act on the `Err`.
pub async fn fire_and_forget(what: &'static str, task: impl Future) {
    let _ = guarded(what, task).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// Read a caller's answer under a hard bound, so a test that leaves the
    /// caller waiting fails rather than hanging.
    async fn answered<T: Send + 'static>(
        response: oneshot::Receiver<Result<T, String>>,
    ) -> Result<T, String> {
        tokio::time::timeout(Duration::from_secs(5), response)
            .await
            .expect("the caller must be answered, not left waiting on dead work")
            .expect("the answering half must not be dropped")
    }

    /// The shape every caller of [`guarded`] has: someone is parked on a
    /// oneshot the detached work was going to answer.
    #[tokio::test]
    async fn a_panicking_detached_task_reaches_its_caller_as_an_error() {
        let (done_tx, done_rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            let outcome = guarded("test detached work", async {
                tokio::task::yield_now().await;
                panic!("the detached work died")
            })
            .await;
            // What the shipped sites do with the `Err`: answer the caller with it.
            let _ = done_tx.send(outcome.map(|()| unreachable!("a panicking round finishes")));
        });

        let error = answered(done_rx).await.unwrap_err();
        assert!(
            error.contains("the detached work died"),
            "the panic's own message must reach the caller, got {error:?}"
        );
        join.await
            .expect("the observing task must not itself panic");
    }

    /// A formatted payload arrives verbatim, so the caller can tell the
    /// difference between ways the same task can die.
    #[tokio::test]
    async fn a_formatted_payload_reaches_the_caller_verbatim() {
        let outcome = guarded("test detached work", async {
            panic!("{} server went away", "roslyn");
        })
        .await;
        assert_eq!(
            outcome.map(|()| unreachable!("the round panicked")),
            Err("roslyn server went away".to_string())
        );
    }

    /// A payload that is not a message at all is still reported, and still
    /// ends the await rather than losing the task.
    #[tokio::test]
    async fn a_non_message_payload_is_still_reported() {
        let outcome = guarded("test detached work", async {
            std::panic::panic_any(7u8);
        })
        .await;
        assert_eq!(
            outcome.map(|()| unreachable!("the round panicked")),
            Err("panicked with a payload that is not a message".to_string())
        );
    }

    /// The success path is transparent: whatever the detached future produced
    /// is what its caller gets.
    #[tokio::test]
    async fn a_completed_task_passes_its_value_through() {
        assert_eq!(
            guarded("test detached work", async { 40 + 2 }).await,
            Ok(42)
        );
        let outcome = guarded("test detached work", async { "finished" }).await;
        assert_eq!(outcome.map(str::to_string), Ok("finished".to_string()));
    }

    /// The point of [`fire_and_forget`] is that the panic stops at the task: a
    /// bare `tokio::spawn` of the same work answers its spawner with a join
    /// failure, which is the loss this whole file exists to prevent.
    #[tokio::test]
    async fn a_panicking_fire_and_forget_task_ends_without_a_join_failure() {
        let survived = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::spawn(fire_and_forget("test fire and forget", async {
                tokio::task::yield_now().await;
                panic!("the detached work died");
            })),
        )
        .await
        .expect("the wrapper must finish the task rather than hang")
        .expect("a panicked fire-and-forget task must not fail its join handle");
        assert_eq!(survived, ());
    }
}
