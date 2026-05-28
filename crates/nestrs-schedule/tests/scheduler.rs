//! Drive the `Scheduler` transport end-to-end against a hand-built container,
//! exercising all three triggers live. Metadata is attached directly
//! (`attach_meta` only needs a `'static` host type), so the test needs neither
//! the `#[cron_job]` macro nor a full module tree — just the public surface.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nestrs_core::{Container, Transport};
use nestrs_schedule::{CronExpression, CronJobMeta, Scheduler, Trigger};
use tokio_util::sync::CancellationToken;

static INTERVAL_HITS: AtomicU64 = AtomicU64::new(0);
static TIMEOUT_HITS: AtomicU64 = AtomicU64::new(0);
static CRON_HITS: AtomicU64 = AtomicU64::new(0);

type RunFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

fn tick_interval(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        INTERVAL_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

fn tick_timeout(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        TIMEOUT_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

fn tick_cron(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        CRON_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler_runs_interval_timeout_and_cron_jobs() {
    struct IntervalHost;
    struct TimeoutHost;
    struct CronHost;

    let container = Container::builder()
        .attach_meta::<IntervalHost, CronJobMeta>(CronJobMeta {
            name: "interval",
            trigger: Trigger::Interval(Duration::from_millis(200)),
            run: tick_interval,
        })
        .attach_meta::<TimeoutHost, CronJobMeta>(CronJobMeta {
            name: "timeout",
            trigger: Trigger::Timeout(Duration::from_millis(300)),
            run: tick_timeout,
        })
        .attach_meta::<CronHost, CronJobMeta>(CronJobMeta {
            name: "cron",
            trigger: Trigger::Cron {
                expr: CronExpression::EVERY_SECOND,
                tz: None,
            },
            run: tick_cron,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));

    // ~2.2s: interval (200ms) fires ~10×, the one-shot fires once at 300ms, and
    // the per-second cron crosses at least one whole-second boundary.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(
        INTERVAL_HITS.load(Ordering::SeqCst) >= 2,
        "interval job fires repeatedly",
    );
    assert_eq!(
        TIMEOUT_HITS.load(Ordering::SeqCst),
        1,
        "one-shot job fires exactly once",
    );
    assert!(
        CRON_HITS.load(Ordering::SeqCst) >= 1,
        "cron job fires at least once",
    );
}

/// A malformed cron expression must abort the boot at configure time, naming the
/// offending job — not fail silently or only on the first fire.
#[tokio::test]
async fn invalid_cron_expression_fails_configure() {
    struct BadHost;

    let container = Container::builder()
        .attach_meta::<BadHost, CronJobMeta>(CronJobMeta {
            name: "broken",
            trigger: Trigger::Cron {
                expr: "not a cron expression",
                tz: None,
            },
            run: tick_cron,
        })
        .build();

    let err = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("an invalid cron expression aborts configure");
    assert!(
        err.to_string().contains("broken"),
        "the error names the offending job: {err}",
    );
}
