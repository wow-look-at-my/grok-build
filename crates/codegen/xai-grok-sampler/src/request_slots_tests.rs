use super::*;

use std::sync::atomic::AtomicUsize;

/// Run `count` holders against `slots` and report the most that held a slot
/// at the same time. Each holder keeps its slot for `hold`.
async fn peak_concurrency(slots: &Arc<RequestSlots>, count: usize, hold: Duration) -> usize {
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..count {
        let slots = Arc::clone(slots);
        let live = Arc::clone(&live);
        let peak = Arc::clone(&peak);
        tasks.spawn(async move {
            let _slot = slots.acquire().await;
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(hold).await;
            live.fetch_sub(1, Ordering::SeqCst);
        });
    }
    while let Some(joined) = tasks.join_next().await {
        joined.expect("a holder finished");
    }
    peak.load(Ordering::SeqCst)
}

#[tokio::test(start_paused = true)]
async fn the_default_cap_is_seven() {
    let slots = Arc::new(RequestSlots::new(DEFAULT_MAX_PARALLEL_REQUESTS));
    assert_eq!(
        peak_concurrency(&slots, 20, Duration::from_millis(50)).await,
        7
    );
}

#[tokio::test(start_paused = true)]
async fn requests_past_the_cap_queue_and_all_run() {
    let slots = Arc::new(RequestSlots::new(3));
    assert_eq!(
        peak_concurrency(&slots, 12, Duration::from_millis(50)).await,
        3
    );
    assert_eq!(slots.waiting(), 0);
}

#[tokio::test(start_paused = true)]
async fn zero_is_no_cap() {
    let slots = Arc::new(RequestSlots::new(0));
    assert_eq!(slots.limit(), 0);
    assert_eq!(
        peak_concurrency(&slots, 40, Duration::from_millis(50)).await,
        40
    );
}

#[tokio::test(start_paused = true)]
async fn the_queue_is_first_in_first_out() {
    let slots = Arc::new(RequestSlots::new(1));
    let first = slots.acquire().await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..5 {
        let waiter = Arc::clone(&slots);
        let order = Arc::clone(&order);
        tasks.spawn(async move {
            let _slot = waiter.acquire().await;
            order.lock().unwrap().push(n);
        });
        // Each waiter joins the queue before the next one starts.
        while slots.waiting() <= n {
            tokio::task::yield_now().await;
        }
    }
    drop(first);
    while tasks.join_next().await.is_some() {}
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4]);
}

#[tokio::test(start_paused = true)]
async fn a_raised_limit_lets_the_queue_through() {
    let slots = Arc::new(RequestSlots::new(1));
    let held = slots.acquire().await;
    let waiter = {
        let slots = Arc::clone(&slots);
        tokio::spawn(async move { slots.acquire().await })
    };
    while slots.waiting() == 0 {
        tokio::task::yield_now().await;
    }
    slots.set_limit(2);
    let second = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("a raised limit must admit the waiter")
        .expect("the waiter finished");
    drop((held, second));
}

#[tokio::test(start_paused = true)]
async fn a_lowered_limit_takes_effect_as_slots_come_back() {
    let slots = Arc::new(RequestSlots::new(4));
    let held: Vec<_> = futures_util::future::join_all((0..4).map(|_| slots.acquire())).await;
    slots.set_limit(2);
    assert_eq!(slots.limit(), 2);
    drop(held);
    assert_eq!(
        peak_concurrency(&slots, 10, Duration::from_millis(50)).await,
        2
    );
    // Raising it back must not double-count the permits the shrink took.
    slots.set_limit(5);
    assert_eq!(
        peak_concurrency(&slots, 10, Duration::from_millis(50)).await,
        5
    );
}

#[tokio::test(start_paused = true)]
async fn a_dropped_waiter_leaves_the_queue() {
    let slots = Arc::new(RequestSlots::new(1));
    let held = slots.acquire().await;
    let waiter = {
        let slots = Arc::clone(&slots);
        tokio::spawn(async move { slots.acquire().await })
    };
    while slots.waiting() == 0 {
        tokio::task::yield_now().await;
    }
    waiter.abort();
    let _ = waiter.await;
    assert_eq!(slots.waiting(), 0);
    drop(held);
    let _next = tokio::time::timeout(Duration::from_secs(1), slots.acquire())
        .await
        .expect("an aborted waiter must not keep the slot");
}

/// The request queues for longer than the deadline, then runs for less than it.
#[tokio::test(start_paused = true)]
async fn queue_time_does_not_count_against_the_timeout() {
    let slots = Arc::new(RequestSlots::new(1));
    let held = slots.acquire().await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(held);
    });
    let result = timeout_excluding_queue(Duration::from_secs(10), async {
        let _slot = slots.acquire().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        "answered"
    })
    .await;
    assert_eq!(result, Ok("answered"));
    release.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_plain_timeout_counts_the_queue() {
    let slots = Arc::new(RequestSlots::new(1));
    let _held = slots.acquire().await;
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let _slot = slots.acquire().await;
    })
    .await;
    assert!(result.is_err());
}

/// Time after the slot is granted still counts in full.
#[tokio::test(start_paused = true)]
async fn run_time_still_counts_against_the_timeout() {
    let slots = Arc::new(RequestSlots::new(1));
    let held = slots.acquire().await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(held);
    });
    let started = Instant::now();
    let result = timeout_excluding_queue(Duration::from_secs(10), async {
        let _slot = slots.acquire().await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    })
    .await;
    let err = result.expect_err("60 s of run time is past a 10 s limit");
    assert_eq!(err.queued, Duration::from_secs(30));
    assert_eq!(started.elapsed(), Duration::from_secs(40));
    release.await.unwrap();
}

/// An outer deadline also excludes a wait that an inner one recorded.
#[tokio::test(start_paused = true)]
async fn nested_timeouts_all_exclude_the_queue() {
    let slots = Arc::new(RequestSlots::new(1));
    let held = slots.acquire().await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(held);
    });
    let result = timeout_excluding_queue(Duration::from_secs(10), async {
        timeout_excluding_queue(Duration::from_secs(20), async {
            let _slot = slots.acquire().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await
    })
    .await;
    assert_eq!(result, Ok(Ok(())));
    release.await.unwrap();
}

/// A request that runs on another task records into the caller's clock through `in_queue_clock`.
#[tokio::test(start_paused = true)]
async fn a_spawned_request_records_into_the_callers_clock() {
    let slots = Arc::new(RequestSlots::new(1));
    let held = slots.acquire().await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(held);
    });
    let result = timeout_excluding_queue(Duration::from_secs(10), async {
        let clock = current_queue_clock();
        let slots = Arc::clone(&slots);
        tokio::spawn(in_queue_clock(clock, async move {
            let _slot = slots.acquire().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }))
        .await
        .unwrap();
    })
    .await;
    assert_eq!(result, Ok(()));
    release.await.unwrap();
}

#[tokio::test]
async fn a_held_slot_is_not_taken_twice() {
    let taken = with_slot_held(acquire_unless_held()).await;
    assert!(taken.is_none());
}
