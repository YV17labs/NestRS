//! [`JobConsumer`] — what a queue backend implements so the port's
//! [`QueueWorker`](crate::QueueWorker) runs its jobs — the values its calls
//! trade ([`Ask`], [`Received`], [`Prepared`], [`LeaseHold`]), and
//! [`BoundConsumer`], the one a backend's binding declares.

use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::worker::{Run, Serving};
use crate::{
    CheckpointStore, Delivery, Disposition, JobId, ProcessMethod, QueueBackend, QueueError,
};

/// What a queue backend implements so the port's
/// [`QueueWorker`](crate::QueueWorker) runs its jobs.
///
/// The worker owns what every backend does alike — each method's permits, one
/// task per delivery, the renewal cadence, the attempt and its budget, the
/// drain — and calls these for what only the backend knows: how its storage
/// hands a job over, keeps it leased, and ends its delivery.
///
/// # Contract
///
/// - **[`receive`](Self::receive)** hands over at most `ask.max` deliveries of
///   the method's queue, each leased to this worker in the step that hands it
///   over, and waits at most `ask.wait` for the first. Each carries its
///   record's delivery count — 1 the first time the record is handed over, one
///   more each time it is handed over again — and how long its lease lasts.
/// - **[`renew`](Self::renew)** extends each lease the backend still holds for
///   this worker, never counting a delivery, and answers one [`LeaseHold`] per
///   lease, in order.
/// - **[`settle`](Self::settle)** ends one delivery as its [`Disposition`]
///   says — in the step that checks the backend still holds it for this worker —
///   and answers [`LeaseHold::Lost`] when it does not. A backend that files the
///   job's next record before it lets the delivery go may leave that record
///   behind on `Lost`: a second delivery, which at least once allows.
/// - **Every call is bounded by the backend's own budget**, which
///   [`Prepared::answer_bound`] states; the port's net
///   ([`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT)) only answers a backend that
///   stopped bounding itself.
///
/// Every call is made on the worker's runtime, so each future is `Send`.
pub trait JobConsumer: Send + Sync + 'static {
    /// The backend's proof that it holds a delivery for this worker — what
    /// [`renew`](Self::renew) and [`settle`](Self::settle) check. The port
    /// carries it and never reads it.
    type Lease: Send + Sync + 'static;

    /// The declaration the backend's producer reads too: its name and the
    /// optional capabilities it honours.
    fn backend(&self) -> &'static QueueBackend;

    /// Ready the storage for `methods`, once, at the worker's boot. An error
    /// fails the boot.
    fn prepare(
        &self,
        methods: &[&'static ProcessMethod],
    ) -> impl Future<Output = Result<Prepared, QueueError>> + Send;

    /// At most `ask.max` deliveries of `method`'s queue, waiting at most
    /// `ask.wait` for the first — none, when that much time passed with none.
    fn receive(
        &self,
        method: &'static ProcessMethod,
        ask: Ask,
    ) -> impl Future<Output = Result<Received<Self::Lease>, QueueError>> + Send;

    /// Extend each of `leases`, held by deliveries of `method`, without counting
    /// a delivery; one answer per lease, in order.
    fn renew(
        &self,
        method: &'static ProcessMethod,
        leases: &[&Self::Lease],
    ) -> impl Future<Output = Result<Vec<LeaseHold>, QueueError>> + Send;

    /// End the delivery `lease` holds as `disposition` says, while the backend
    /// still holds it for this worker.
    fn settle(
        &self,
        method: &'static ProcessMethod,
        lease: &Self::Lease,
        disposition: Disposition<'_>,
    ) -> impl Future<Output = Result<LeaseHold, QueueError>> + Send;

    /// Keep `method`'s queue moving while no delivery runs — Redis moves the
    /// delayed jobs that fell due onto the queue — and say when to come back.
    /// `None`, the default, needs no upkeep.
    fn maintain(
        &self,
        method: &'static ProcessMethod,
    ) -> impl Future<Output = Result<Option<Duration>, QueueError>> + Send {
        let _ = method;
        async { Ok(None) }
    }

    /// Let go of what [`prepare`](Self::prepare) opened, at the end of the
    /// worker's drain.
    fn close(&self) -> impl Future<Output = Result<(), QueueError>> + Send {
        async { Ok(()) }
    }

    /// Where the delivery `lease` holds keeps job `job`'s checkpoint, fenced on
    /// the lease — for a backend declaring
    /// [`Capability::Checkpoint`](crate::Capability::Checkpoint). The port
    /// refuses a method declaring a checkpoint on a backend without it, and
    /// treats `None` from one declaring it as the driver defect it is.
    fn checkpoint(
        &self,
        method: &'static ProcessMethod,
        lease: &Self::Lease,
        job: &JobId,
    ) -> Option<Arc<dyn CheckpointStore>> {
        let _ = (method, lease, job);
        None
    }
}

/// How much a [`receive`](JobConsumer::receive) may hand over, and how long it
/// may wait for the first delivery. Built by the port.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Ask {
    /// The most deliveries to hand over: the method's free permits.
    pub max: NonZeroU32,
    /// The longest to wait for the first.
    pub wait: Duration,
}

impl Ask {
    pub(crate) const fn new(max: NonZeroU32, wait: Duration) -> Self {
        Self { max, wait }
    }
}

/// What a [`receive`](JobConsumer::receive) handed over.
#[derive(Debug)]
#[non_exhaustive]
pub struct Received<L> {
    /// The deliveries, each holding its lease.
    pub deliveries: Vec<Delivery<L>>,
    /// How long the method's throttle holds every further start, when its
    /// window is full.
    pub throttled_for: Option<Duration>,
}

impl<L> Received<L> {
    /// `deliveries`, with no throttle holding the next.
    pub fn new(deliveries: Vec<Delivery<L>>) -> Self {
        Self {
            deliveries,
            throttled_for: None,
        }
    }

    /// The method's throttle holds every further start for `window`.
    #[must_use]
    pub fn throttled_for(mut self, window: Duration) -> Self {
        self.throttled_for = Some(window);
        self
    }
}

/// What [`prepare`](JobConsumer::prepare) tells the port about the backend.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Prepared {
    /// The longest one call takes to answer or fail on its own — the backend's
    /// budget — from which the drain keeps its hand-back reserve.
    pub answer_bound: Duration,
}

impl Prepared {
    /// A backend whose calls answer or fail within `answer_bound`.
    pub const fn new(answer_bound: Duration) -> Self {
        Self { answer_bound }
    }
}

/// Whether the backend still holds a delivery for this worker — the answer of
/// [`renew`](JobConsumer::renew) and [`settle`](JobConsumer::settle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeaseHold {
    /// The backend holds it for this worker: renewed, or settled as asked.
    Held,
    /// It does not — another delivery of the job holds it, or the job ended —
    /// and nothing was written.
    Lost,
}

/// A queue backend's consumer, as its binding declares it for the port's
/// [`QueueWorker`](crate::QueueWorker).
///
/// Built from any [`JobConsumer`] and declared with
/// `ContainerBuilder::provide_declared_factory_after`, carrying
/// [`BACKEND_REMEDY`](crate::BACKEND_REMEDY): two backends imported is a boot
/// error naming both, as for `Arc<dyn JobProducer>`. The consumer keeps its own
/// type behind it, so the worker's loop calls it without a box per job.
pub struct BoundConsumer {
    run: Box<dyn Run>,
}

impl BoundConsumer {
    /// The consumer the worker runs for this app.
    pub fn new<C: JobConsumer>(consumer: C) -> Self {
        Self {
            run: Box::new(Arc::new(consumer)),
        }
    }

    pub(crate) fn backend(&self) -> &'static QueueBackend {
        self.run.backend()
    }

    pub(crate) fn prepare<'a>(
        &'a self,
        methods: &'a [&'static ProcessMethod],
    ) -> BoxFuture<'a, Result<Prepared, QueueError>> {
        self.run.prepare(methods)
    }

    pub(crate) fn serve(&self, serving: Serving) -> BoxFuture<'_, ()> {
        self.run.serve(serving)
    }
}

impl fmt::Debug for BoundConsumer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundConsumer")
            .field("backend", &self.backend().name())
            .finish()
    }
}
