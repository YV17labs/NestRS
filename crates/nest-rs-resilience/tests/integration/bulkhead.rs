use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_resilience::{Bulkhead, Policy, ResilienceError, code};
use tokio::sync::oneshot;

#[derive(Debug)]
struct Never;

#[tokio::test(start_paused = true)]
async fn a_call_past_the_slots_is_refused_and_a_freed_slot_is_reused() {
    let policy = Policy::builder("narrow")
        .bulkhead(Bulkhead::new(NonZeroU32::MIN))
        .build()
        .unwrap();
    let (release, released) = oneshot::channel::<()>();
    let holder = {
        let policy = policy.clone();
        tokio::spawn(async move {
            let mut released = Some(released);
            policy
                .run(move |_| {
                    let released = released.take();
                    async move {
                        if let Some(released) = released {
                            released.await.unwrap();
                        }
                        Ok::<(), Never>(())
                    }
                })
                .await
        })
    };
    tokio::task::yield_now().await;
    assert_eq!(policy.available_slots(), Some(0));

    let refused = policy
        .run(|_| async { Ok::<(), Never>(()) })
        .await
        .unwrap_err();
    assert!(matches!(refused, ResilienceError::BulkheadFull { .. }));
    assert_eq!(refused.problem().unwrap().code(), code::BULKHEAD_FULL);

    release.send(()).unwrap();
    holder.await.unwrap().unwrap();
    assert_eq!(policy.available_slots(), Some(1));
    policy.run(|_| async { Ok::<(), Never>(()) }).await.unwrap();
    assert_eq!(policy.stats().bulkhead_rejected, 1);
}

#[tokio::test(start_paused = true)]
async fn a_call_queues_at_most_max_wait() {
    let policy = Policy::builder("queued")
        .bulkhead(Bulkhead::new(NonZeroU32::MIN).with_max_wait(Duration::from_millis(200)))
        .build()
        .unwrap();
    let holder = {
        let policy = policy.clone();
        tokio::spawn(async move {
            policy
                .run(|_| async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok::<(), Never>(())
                })
                .await
        })
    };
    tokio::task::yield_now().await;
    // The holder frees its slot within the wait: the queued call runs.
    policy.run(|_| async { Ok::<(), Never>(()) }).await.unwrap();
    holder.await.unwrap().unwrap();
}
