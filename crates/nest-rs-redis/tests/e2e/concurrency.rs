//! A `#[process]` method runs at most `concurrency` jobs at once on one replica,
//! against a live worker — one unless it declares more.
//!
//! Each bound is proved from both sides: the peak never climbs past it, and it is
//! reached, because a ceiling nobody reaches reads exactly like a worker that
//! serializes everything. The default is the guard on "one" — a wider dispatch or
//! a `tokio::spawn` inside the attempt climbs past it. Every test has its own
//! queue and its own counters, since nextest runs them side by side on one Redis.
//!
//! Only a live worker shows any of it: the port's suite runs one attempt at a
//! time, which is exactly the condition under which a lost bound stays invisible.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisWorkerModule};
use serde::{Deserialize, Serialize};

/// Simultaneous handler bodies, the peak seen, and the jobs finished — one set
/// per test. Process-wide statics: the container owns the provider, and the
/// assertion runs outside it.
struct Gauge {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    ran: AtomicUsize,
}

impl Gauge {
    const fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            ran: AtomicUsize::new(0),
        }
    }

    /// Hold one slot for `hold`: while it is held, a job the bound lets through
    /// starts beside it and the peak climbs.
    async fn hold(&self, hold: Duration) {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(hold).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.ran.fetch_add(1, Ordering::SeqCst);
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn ran(&self) -> usize {
        self.ran.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HoldCommand {
    seq: usize,
}

// --- the default: one at a time ------------------------------------------------

static SERIAL: Gauge = Gauge::new();

#[queue(name = "nestrs-e2e-concurrency-default", job = HoldCommand)]
struct SerialQueue;

#[injectable]
#[derive(Default)]
struct SerialProcessor;

#[processor]
impl SerialProcessor {
    #[process(queue = SerialQueue, retries = 0)]
    async fn hold(&self, _job: HoldCommand) -> anyhow::Result<()> {
        SERIAL.hold(Duration::from_millis(250)).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [SerialProcessor],
)]
struct SerialModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_method_runs_one_job_at_a_time_unless_it_declares_more() {
    const JOBS: usize = 6;
    let serial = crate::replica::<SerialModule>().await;
    for seq in 0..JOBS {
        serial
            .producer
            .push(SerialQueue, HoldCommand { seq }, None)
            .await
            .expect("enqueue");
    }
    crate::wait_until(Duration::from_secs(15), || SERIAL.ran() == JOBS).await;
    serial
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(SERIAL.ran(), JOBS, "every job drains, one after another");
    assert_eq!(
        SERIAL.peak(),
        1,
        "a method declaring no concurrency runs one job at a time, but {} ran at once",
        SERIAL.peak(),
    );
}

// --- a declared bound ----------------------------------------------------------

static BOUNDED: Gauge = Gauge::new();

#[queue(name = "nestrs-e2e-concurrency-three", job = HoldCommand)]
struct BoundedQueue;

#[injectable]
#[derive(Default)]
struct BoundedProcessor;

#[processor]
impl BoundedProcessor {
    #[process(queue = BoundedQueue, retries = 0, concurrency = 3)]
    async fn hold(&self, _job: HoldCommand) -> anyhow::Result<()> {
        BOUNDED.hold(Duration::from_millis(800)).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [BoundedProcessor],
)]
struct BoundedModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_concurrency_is_reached_and_never_exceeded() {
    const JOBS: usize = 9;
    let bounded = crate::replica::<BoundedModule>().await;
    for seq in 0..JOBS {
        bounded
            .producer
            .push(BoundedQueue, HoldCommand { seq }, None)
            .await
            .expect("enqueue");
    }
    crate::wait_until(Duration::from_secs(15), || BOUNDED.ran() == JOBS).await;
    bounded
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(BOUNDED.ran(), JOBS, "every job drains");
    assert_eq!(
        BOUNDED.peak(),
        3,
        "`concurrency = 3` runs three jobs at once and never a fourth",
    );
}
