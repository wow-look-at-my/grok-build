use super::*;
use xai_interjection_core::PendingInterjection;

fn user_message() -> PendingInterjection<agent_client_protocol::ImageContent> {
    PendingInterjection {
        text: "user message".to_owned(),
        attachments: Vec::new(),
    }
}

/// The `biased` select prefers an already-finished wait result over either interrupt.
async fn race_wait(
    human: &InterjectionBuffer<agent_client_protocol::ImageContent>,
    parent: &ParentInterjectSignal,
    wait: impl Future<Output = &'static str>,
) -> Result<&'static str, WaitInterruptCause> {
    tokio::select! {
        biased;
        result = wait => Ok(result),
        cause = wait_for_wait_interrupt(human, parent) => Err(cause),
    }
}

async fn never_finishes() -> &'static str {
    tokio::time::sleep(Duration::from_secs(3600)).await;
    "wait-result"
}

#[tokio::test(start_paused = true)]
async fn human_interjection_aborts_an_in_flight_wait() {
    let human = InterjectionBuffer::new();
    let parent = ParentInterjectSignal::default();
    human.push(user_message());
    assert_eq!(
        Err(WaitInterruptCause::HumanInterjection),
        race_wait(&human, &parent, never_finishes()).await
    );
    assert_eq!(
        Ok("wait-result"),
        race_wait(&human, &parent, async { "wait-result" }).await
    );
}

#[tokio::test(start_paused = true)]
async fn parent_interject_signal_aborts_an_in_flight_wait() {
    let human = InterjectionBuffer::new();
    let parent = ParentInterjectSignal::default();
    parent.set_pending(true);
    assert_eq!(
        Err(WaitInterruptCause::ParentInterject),
        race_wait(&human, &parent, never_finishes()).await
    );
    parent.set_pending(false);
    let unresolved = tokio::time::timeout(
        Duration::from_millis(500),
        race_wait(&human, &parent, never_finishes()),
    )
    .await;
    assert!(
        unresolved.is_err(),
        "a cleared signal must not abort the wait"
    );
}

#[tokio::test(start_paused = true)]
async fn human_cause_wins_when_both_are_pending() {
    let human = InterjectionBuffer::new();
    human.push(user_message());
    let parent = ParentInterjectSignal::default();
    parent.set_pending(true);
    assert_eq!(
        Err(WaitInterruptCause::HumanInterjection),
        race_wait(&human, &parent, never_finishes()).await
    );
}
