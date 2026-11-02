use super::*;

#[tokio::test]
async fn spawn_returns_the_output() {
    let v = spawn(async { 41 + 1 }).await;
    assert_eq!(v.ok(), Some(42), "the spawned future's output must arrive");
}

#[tokio::test]
async fn spawn_panic_is_a_join_error() {
    let res = spawn(async { panic!("boom") }).await;
    let err = res.err();
    assert!(
        err.as_ref().is_some_and(tokio::task::JoinError::is_panic),
        "a panicked task must surface as a panic JoinError, got {err:?}"
    );
}

#[tokio::test]
async fn spawn_blocking_returns_the_value() {
    let v = spawn_blocking(|| String::from("done")).await;
    assert_eq!(v.ok().as_deref(), Some("done"));
}

#[tokio::test]
async fn spawn_blocking_panic_is_a_join_error() {
    let res = spawn_blocking(|| -> u8 { panic!("boom") }).await;
    assert!(res.err().is_some_and(|e| e.is_panic()));
}

#[tokio::test]
async fn spawn_on_and_blocking_on_use_the_handle() {
    let handle = Handle::current();
    let a = spawn_on(&handle, async { 1u8 }).await;
    let b = spawn_blocking_on(&handle, || 2u8).await;
    assert_eq!((a.ok(), b.ok()), (Some(1), Some(2)));
}

#[tokio::test]
async fn spawn_in_adds_to_the_set() {
    let mut set = JoinSet::new();
    spawn_in(&mut set, async { 7u32 });
    spawn_in(&mut set, async { 8u32 });
    let mut got = Vec::new();
    while let Some(r) = set.join_next().await {
        got.push(r.ok());
    }
    got.sort();
    assert_eq!(got, vec![Some(7), Some(8)]);
}

#[tokio::test]
async fn spawn_local_runs_a_non_send_future() {
    let local = tokio::task::LocalSet::new();
    let v = local
        .run_until(async {
            let rc = std::rc::Rc::new(5);
            spawn_local(async move { *rc * 2 }).await
        })
        .await;
    assert_eq!(v.ok(), Some(10));
}
