//! The port's worker, in process: the behaviour kit over an in-memory backend
//! that files a record due later and over one that does not — where the worker
//! waits a retry's backoff itself — and what the worker refuses at boot.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{Transport, injectable, module};
use nest_rs_queue::{BoundConsumer, JobProducer, QueueName, QueueWorker, processor, queue};
use nest_rs_testing::TestApp;
use nest_rs_testing::queue::KitBackend;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::memory::{DELAYING, Memory, PLAIN};

/// How long an in-memory delivery's lease lasts in the kit.
const LEASE: Duration = Duration::from_millis(500);

/// The kit's backend over one in-memory store, shared by every app a case
/// builds.
struct MemoryKit {
    memory: Memory,
}

impl MemoryKit {
    fn delaying() -> Self {
        Self {
            memory: Memory::new(&DELAYING, LEASE),
        }
    }

    fn plain() -> Self {
        Self {
            memory: Memory::new(&PLAIN, LEASE),
        }
    }
}

impl KitBackend for MemoryKit {
    fn app(&self) -> nest_rs_testing::TestAppBuilder {
        let producer: Arc<dyn JobProducer> = Arc::new(self.memory.clone());
        TestApp::builder()
            .provide(BoundConsumer::new(self.memory.consumer()))
            .provide_dyn(producer)
    }

    fn lease(&self) -> Duration {
        LEASE
    }

    async fn purge(&self, queue: &QueueName) -> anyhow::Result<()> {
        self.memory.purge(queue.as_str());
        Ok(())
    }

    async fn lapse(&self, queue: &QueueName) -> anyhow::Result<()> {
        self.memory.lapse(queue.as_str());
        Ok(())
    }

    async fn take(&self, queue: &QueueName) -> anyhow::Result<()> {
        self.memory.take(queue.as_str());
        Ok(())
    }
}

/// The kit over a backend that files a record due later.
mod delaying {
    use super::MemoryKit;

    nest_rs_testing::queue_kit!(MemoryKit::delaying());
}

/// The kit over a backend that files nothing due later: a retry's backoff is
/// the worker's to wait, holding its permit and renewing its lease.
mod plain {
    use super::MemoryKit;

    nest_rs_testing::queue_kit!(MemoryKit::plain());
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkerCommand {
    seq: u32,
}

#[queue(name = "nestrs-worker-boot", job = WorkerCommand)]
struct BootQueue;

#[injectable]
#[derive(Default)]
struct BootProcessor;

#[processor]
impl BootProcessor {
    #[process(queue = BootQueue)]
    async fn run(&self, _job: WorkerCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(providers = [BootProcessor])]
struct BootModule;

#[module]
struct EmptyModule;

/// A reachable `#[process]` method with no queue backend bound to run it fails
/// the boot, naming the method, its queue and the remedy.
#[tokio::test]
async fn a_method_with_no_backend_bound_fails_the_boot_naming_the_remedy() {
    let app = TestApp::builder()
        .module::<BootModule>()
        .build_headless()
        .await
        .expect("the app boots");
    let refused = QueueWorker::new()
        .configure(app.container())
        .await
        .expect_err("no backend runs the method")
        .to_string();
    for expected in [
        "BootProcessor::run",
        "nestrs-worker-boot",
        nest_rs_queue::BACKEND_REMEDY,
    ] {
        assert!(refused.contains(expected), "{expected:?} in {refused}");
    }
}

/// A producer bound on one backend and a consumer on another fail the boot,
/// naming both: the jobs pushed would never reach the worker.
#[tokio::test]
async fn a_producer_and_a_consumer_of_two_backends_fail_the_boot_naming_both() {
    let producer: Arc<dyn JobProducer> = Arc::new(Memory::new(&PLAIN, LEASE));
    let consumer = Memory::new(&DELAYING, LEASE).consumer();
    let app = TestApp::builder()
        .module::<BootModule>()
        .provide(BoundConsumer::new(consumer))
        .provide_dyn(producer)
        .build_headless()
        .await
        .expect("the app boots");
    let refused = QueueWorker::new()
        .configure(app.container())
        .await
        .expect_err("two backends")
        .to_string();
    for expected in ["memory-plain", "`memory`"] {
        assert!(refused.contains(expected), "{expected:?} in {refused}");
    }
}

/// With no method reachable the worker needs no backend: it starts, idles, and
/// stops at once on the signal.
#[tokio::test]
async fn a_worker_with_nothing_to_run_idles_and_stops_at_once() {
    let app = TestApp::builder()
        .module::<EmptyModule>()
        .build_headless()
        .await
        .expect("the app boots");
    let mut worker = QueueWorker::new();
    worker
        .configure(app.container())
        .await
        .expect("nothing to run is no error");
    let stop = CancellationToken::new();
    let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(1), serving)
        .await
        .expect("an idle worker stops at once")
        .expect("its task ends")
        .expect("cleanly");
}

/// A worker over `memory` serving [`BootModule`], with `memory`'s producer.
async fn boot_worker(
    memory: &Memory,
) -> (
    nest_rs_testing::HeadlessApp,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let producer: Arc<dyn JobProducer> = Arc::new(memory.clone());
    let app = TestApp::builder()
        .module::<BootModule>()
        .provide(BoundConsumer::new(memory.consumer()))
        .provide_dyn(producer)
        .build_headless()
        .await
        .expect("the app boots");
    let mut worker = QueueWorker::new();
    worker
        .configure(app.container())
        .await
        .expect("the worker configures");
    let stop = CancellationToken::new();
    let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
    (app, stop, serving)
}

/// Wait until `done` holds, for at most ten seconds.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A record that is not JSON is dead-lettered on its own, kept as stored, with
/// a reason that says where it failed and never what it held — and the job
/// filed beside it runs; once both ended, the queue holds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_is_not_json_is_dead_lettered_alone_without_its_value() {
    let memory = Memory::new(&DELAYING, LEASE);
    let queue = "nestrs-worker-boot";
    memory.file_raw(queue, b"secret-token-not-json".to_vec());
    let (app, stop, serving) = boot_worker(&memory).await;
    let producer = app
        .container()
        .get_dyn::<dyn JobProducer>()
        .expect("the producer");
    nest_rs_queue::JobProducerExt::push(&*producer, BootQueue, WorkerCommand { seq: 1 }, None)
        .await
        .expect("a push");
    until("both records ended", || {
        memory.dead(queue).len() == 1 && memory.held(queue) == 0
    })
    .await;
    stop.cancel();
    serving.await.expect("ends").expect("cleanly");
    let dead = memory.dead(queue);
    assert_eq!(dead[0].record, b"secret-token-not-json", "kept as stored");
    assert!(
        !dead[0].reason.contains("secret-token"),
        "the reason never quotes the record: {}",
        dead[0].reason
    );
}

/// A consumer over `memory` down for its first `failures` settles: its
/// renewals fail meanwhile too.
struct Flaky {
    inner: crate::memory::MemoryConsumer,
    failures: std::sync::atomic::AtomicU32,
}

impl nest_rs_queue::JobConsumer for Flaky {
    type Lease = crate::memory::MemoryLease;

    fn backend(&self) -> &'static nest_rs_queue::QueueBackend {
        self.inner.backend()
    }

    async fn prepare(
        &self,
        methods: &[&'static nest_rs_queue::ProcessMethod],
    ) -> Result<nest_rs_queue::Prepared, nest_rs_queue::QueueError> {
        self.inner.prepare(methods).await
    }

    async fn receive(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        ask: nest_rs_queue::Ask,
    ) -> Result<nest_rs_queue::Received<Self::Lease>, nest_rs_queue::QueueError> {
        self.inner.receive(method, ask).await
    }

    async fn renew(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        leases: &[&Self::Lease],
    ) -> Result<Vec<nest_rs_queue::LeaseHold>, nest_rs_queue::QueueError> {
        // Down for the settle is down for the renewal too.
        if self.failures.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            return Err(nest_rs_queue::QueueError::backend(std::io::Error::other(
                "the backend is unreachable",
            )));
        }
        self.inner.renew(method, leases).await
    }

    async fn settle(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        lease: &Self::Lease,
        disposition: nest_rs_queue::Disposition<'_>,
    ) -> Result<nest_rs_queue::LeaseHold, nest_rs_queue::QueueError> {
        let left = self.failures.load(std::sync::atomic::Ordering::Relaxed);
        if left > 0 {
            self.failures
                .store(left - 1, std::sync::atomic::Ordering::Relaxed);
            return Err(nest_rs_queue::QueueError::backend(std::io::Error::other(
                "the backend is unreachable",
            )));
        }
        self.inner.settle(method, lease, disposition).await
    }

    async fn maintain(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
    ) -> Result<Option<Duration>, nest_rs_queue::QueueError> {
        self.inner.maintain(method).await
    }
}

/// The lease of the settle cases: long enough for two retries of a settle.
const SETTLE_LEASE: Duration = Duration::from_secs(2);

/// Run one job of [`BootQueue`] over a consumer whose settle fails `failures`
/// times, and return the store once the job's record is gone or `within` passed.
async fn settle_failing(failures: u32, within: Duration) -> Memory {
    let memory = Memory::new(&DELAYING, SETTLE_LEASE);
    let producer: Arc<dyn JobProducer> = Arc::new(memory.clone());
    let app = TestApp::builder()
        .module::<BootModule>()
        .provide(BoundConsumer::new(Flaky {
            inner: memory.consumer(),
            failures: std::sync::atomic::AtomicU32::new(failures),
        }))
        .provide_dyn(Arc::clone(&producer))
        .build_headless()
        .await
        .expect("the app boots");
    let mut worker = QueueWorker::new();
    worker
        .configure(app.container())
        .await
        .expect("the worker configures");
    let stop = CancellationToken::new();
    let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
    nest_rs_queue::JobProducerExt::push(&*producer, BootQueue, WorkerCommand { seq: 0 }, None)
        .await
        .expect("a push");
    let deadline = std::time::Instant::now() + within;
    while memory.held("nestrs-worker-boot") > 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.cancel();
    serving.await.expect("ends").expect("cleanly");
    memory
}

/// A settle that errs is tried again while the lease holds: two failures, then
/// the job's outcome lands, and nothing says it was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settle_that_errs_is_retried_while_its_lease_holds() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let memory = settle_failing(2, Duration::from_secs(5)).await;
    assert_eq!(memory.held("nestrs-worker-boot"), 0, "the outcome landed");
    assert!(
        logs.find(
            nest_rs_queue::TARGET,
            "job outcome not confirmed; unless it was written, the job runs again once its lease lapses",
        )
        .is_empty()
    );
}

/// A settle that keeps failing past the lease is given up, said at `error`, and
/// the job left to its lease — never reported as settled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settle_failing_past_its_lease_is_said_and_the_job_left_to_its_lease() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let memory = settle_failing(u32::MAX, SETTLE_LEASE * 2).await;
    assert_eq!(
        memory.held("nestrs-worker-boot"),
        1,
        "the job is still held"
    );
    // Each delivery the lapsed lease lets run again meets the same backend, and
    // says so again.
    let said = logs.find(
        nest_rs_queue::TARGET,
        "job outcome not confirmed; unless it was written, the job runs again once its lease lapses",
    );
    assert!(!said.is_empty(), "the lost outcome is said");
    assert!(said.iter().all(|line| line.level == "error"));
    assert_eq!(said[0].field("disposition").as_deref(), Some("complete"));
}

/// A consumer over `memory` whose first settle panics — a backend's defect,
/// met inside a delivery's task, outside any attempt.
struct PanicsOnce {
    inner: crate::memory::MemoryConsumer,
    panicked: std::sync::atomic::AtomicBool,
}

impl nest_rs_queue::JobConsumer for PanicsOnce {
    type Lease = crate::memory::MemoryLease;

    fn backend(&self) -> &'static nest_rs_queue::QueueBackend {
        self.inner.backend()
    }

    async fn prepare(
        &self,
        methods: &[&'static nest_rs_queue::ProcessMethod],
    ) -> Result<nest_rs_queue::Prepared, nest_rs_queue::QueueError> {
        self.inner.prepare(methods).await
    }

    async fn receive(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        ask: nest_rs_queue::Ask,
    ) -> Result<nest_rs_queue::Received<Self::Lease>, nest_rs_queue::QueueError> {
        self.inner.receive(method, ask).await
    }

    async fn renew(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        leases: &[&Self::Lease],
    ) -> Result<Vec<nest_rs_queue::LeaseHold>, nest_rs_queue::QueueError> {
        self.inner.renew(method, leases).await
    }

    async fn settle(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        lease: &Self::Lease,
        disposition: nest_rs_queue::Disposition<'_>,
    ) -> Result<nest_rs_queue::LeaseHold, nest_rs_queue::QueueError> {
        assert!(
            self.panicked
                .swap(true, std::sync::atomic::Ordering::SeqCst),
            "the backend's first settle panics",
        );
        self.inner.settle(method, lease, disposition).await
    }

    async fn maintain(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
    ) -> Result<Option<Duration>, nest_rs_queue::QueueError> {
        self.inner.maintain(method).await
    }
}

/// A delivery whose task panics outside its attempt — here in the backend's
/// settle — stops renewing its lease, so the lease lapses and the job runs
/// again: renewed for a task that is gone, it was held for as long as the
/// worker lived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_whose_task_panics_lets_its_lease_lapse() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let lease = Duration::from_millis(500);
    let memory = Memory::new(&DELAYING, lease);
    let producer: Arc<dyn JobProducer> = Arc::new(memory.clone());
    let app = TestApp::builder()
        .module::<BootModule>()
        .provide(BoundConsumer::new(PanicsOnce {
            inner: memory.consumer(),
            panicked: std::sync::atomic::AtomicBool::new(false),
        }))
        .provide_dyn(Arc::clone(&producer))
        .build_headless()
        .await
        .expect("the app boots");
    let mut worker = QueueWorker::new();
    worker
        .configure(app.container())
        .await
        .expect("the worker configures");
    let stop = CancellationToken::new();
    let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
    nest_rs_queue::JobProducerExt::push(&*producer, BootQueue, WorkerCommand { seq: 0 }, None)
        .await
        .expect("a push");
    let deadline = std::time::Instant::now() + lease * 10;
    tokio::time::sleep(lease).await;
    while memory.held("nestrs-worker-boot") > 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let held = memory.held("nestrs-worker-boot");
    stop.cancel();
    serving.await.expect("ends").expect("cleanly");
    assert_eq!(
        held, 0,
        "the job ran again once its lease lapsed, and settled"
    );
    let said = logs.expect_one(
        nest_rs_queue::TARGET,
        "queue delivery panicked outside its attempt; its job is left to its lease",
    );
    assert_eq!(said.level, "error");
    assert_eq!(said.field("queue").as_deref(), Some("nestrs-worker-boot"));
}

/// A consumer over `memory` whose renewals never answer — a backend gone
/// silent under the leases it handed over.
struct Silent {
    inner: crate::memory::MemoryConsumer,
}

impl nest_rs_queue::JobConsumer for Silent {
    type Lease = crate::memory::MemoryLease;

    fn backend(&self) -> &'static nest_rs_queue::QueueBackend {
        self.inner.backend()
    }

    async fn prepare(
        &self,
        methods: &[&'static nest_rs_queue::ProcessMethod],
    ) -> Result<nest_rs_queue::Prepared, nest_rs_queue::QueueError> {
        self.inner.prepare(methods).await
    }

    async fn receive(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        ask: nest_rs_queue::Ask,
    ) -> Result<nest_rs_queue::Received<Self::Lease>, nest_rs_queue::QueueError> {
        self.inner.receive(method, ask).await
    }

    async fn renew(
        &self,
        _method: &'static nest_rs_queue::ProcessMethod,
        _leases: &[&Self::Lease],
    ) -> Result<Vec<nest_rs_queue::LeaseHold>, nest_rs_queue::QueueError> {
        std::future::pending().await
    }

    async fn settle(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
        lease: &Self::Lease,
        disposition: nest_rs_queue::Disposition<'_>,
    ) -> Result<nest_rs_queue::LeaseHold, nest_rs_queue::QueueError> {
        self.inner.settle(method, lease, disposition).await
    }

    async fn maintain(
        &self,
        method: &'static nest_rs_queue::ProcessMethod,
    ) -> Result<Option<Duration>, nest_rs_queue::QueueError> {
        self.inner.maintain(method).await
    }
}

#[queue(name = "nestrs-worker-long", job = WorkerCommand)]
struct LongQueue;

#[injectable]
#[derive(Default)]
struct LongProcessor;

#[processor]
impl LongProcessor {
    #[process(queue = LongQueue)]
    async fn run(&self, _job: WorkerCommand) -> anyhow::Result<()> {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(())
    }
}

#[module(providers = [LongProcessor])]
struct LongModule;

/// A renewal the backend never answers holds no lease past its end: the
/// attempt is cut once its lease lapses, as one no renewal confirmed — not
/// once the port's net gives up on the renewal, long after the backend let
/// another delivery take the job.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renewal_left_unanswered_cuts_its_attempt_when_its_lease_lapses() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let memory = Memory::new(&DELAYING, LEASE);
    let producer: Arc<dyn JobProducer> = Arc::new(memory.clone());
    let app = TestApp::builder()
        .module::<LongModule>()
        .provide(BoundConsumer::new(Silent {
            inner: memory.consumer(),
        }))
        .provide_dyn(Arc::clone(&producer))
        .provide(nest_rs_queue::QueueConfig {
            shutdown_timeout: Duration::from_secs(1),
        })
        .build_headless()
        .await
        .expect("the app boots");
    let mut worker = QueueWorker::new();
    worker
        .configure(app.container())
        .await
        .expect("the worker configures");
    let stop = CancellationToken::new();
    let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
    nest_rs_queue::JobProducerExt::push(&*producer, LongQueue, WorkerCommand { seq: 0 }, None)
        .await
        .expect("a push");
    let cut = "job lease not renewed for a whole lease; its attempt is cut and the job handed back";
    let deadline = std::time::Instant::now() + LEASE * 4;
    while logs.find(nest_rs_queue::TARGET, cut).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the attempt runs on past its {LEASE:?} lease while the renewal waits out the port's \
             {:?} net",
            nest_rs_queue::BACKEND_TIMEOUT,
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.cancel();
    serving.await.expect("ends").expect("cleanly");
}
