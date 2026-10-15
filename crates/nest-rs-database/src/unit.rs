use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt as _;
use futures_util::future::Either;
use nest_rs_core::operation_log::Unit;

use crate::error::Unhonoured;
use crate::executor::{Executor, ExecutorScope, install_unit};

/// What one unit of work is, as the edge running it declares it.
///
/// It can grow within 7.x: build one through [`UnitSpec::new`] and its `with_`
/// methods, never a literal.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct UnitSpec {
    /// The edge's operation-log unit (`ws.message`, `queue.job`, …): the one
    /// name every event the driver files for this unit carries.
    pub unit: Unit,
    /// Request (reads fail closed without an ability) or Job (system work).
    pub scope: ExecutorScope,
    /// One transaction for the unit, or the pool.
    pub policy: TransactionPolicy,
    /// When the unit's body is cut, by the edge running it; `None` for an edge
    /// with no deadline. A failed unit is answered by then at the latest,
    /// whatever its rollback still waits on, and the work held for a commit is
    /// waited for until then; a commit itself is never cut.
    pub deadline: Option<tokio::time::Instant>,
    /// The job an attempt runs, set by the queue edge alone.
    pub job: Option<JobRef>,
}

impl UnitSpec {
    /// A unit of `unit`, run under `scope` and settled by `policy`, with no
    /// deadline and no job.
    pub const fn new(unit: Unit, scope: ExecutorScope, policy: TransactionPolicy) -> Self {
        Self {
            unit,
            scope,
            policy,
            deadline: None,
            job: None,
        }
    }

    /// The same unit, cut at `deadline`.
    #[must_use]
    pub const fn with_deadline(mut self, deadline: tokio::time::Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// The same unit, running one attempt of `job`.
    #[must_use]
    pub fn with_job(mut self, job: JobRef) -> Self {
        self.job = Some(job);
        self
    }
}

/// The job one attempt runs, as the port names it: the queue edge's identity,
/// in no queue crate's types.
///
/// It can grow within 7.x: build one through [`JobRef::new`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRef {
    /// The queue the job was pushed on.
    pub queue: String,
    /// The job's id, the same on every attempt.
    pub id: uuid::Uuid,
    /// Which attempt this is, from 1.
    pub attempt: u32,
}

impl JobRef {
    /// Attempt `attempt` of job `id` on `queue`.
    pub fn new(queue: impl Into<String>, id: uuid::Uuid, attempt: u32) -> Self {
        Self {
            queue: queue.into(),
            id,
            attempt,
        }
    }
}

/// How one unit's data-layer work is settled — for a job, declared on its
/// decorator (`transactional = …`).
///
/// It can grow within 7.x, so a driver's `match` keeps a `_` arm that refuses
/// the unit, answering [`Unhonoured::deterministic`]:
///
/// ```compile_fail,E0004
/// use nest_rs_database::TransactionPolicy;
///
/// fn opens_one(policy: TransactionPolicy) -> bool {
///     match policy {
///         TransactionPolicy::PerUnit => true,
///         TransactionPolicy::Pool => false,
///     }
/// }
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransactionPolicy {
    /// **The default.** One transaction, opened on the unit's first data-layer
    /// touch, committed when the unit succeeds and rolled back otherwise. A
    /// unit that never touches the database never opens one; a read is a
    /// touch.
    #[default]
    PerUnit,
    /// The pool: each statement commits on its own, and the unit owns its
    /// consistency.
    Pool,
}

/// What a boot refusing two unit-of-work bindings tells the reader: a driver's
/// binding declares `Arc<dyn UnitOfWork>` carrying it
/// (`ContainerBuilder::provide_declared_factory`), so two drivers imported
/// side by side fail the boot naming both.
pub const BACKEND_REMEDY: &str = "Import exactly one unit-of-work binding: \
     `nest_rs::seaorm::SeaOrmDatabaseModule` runs every edge's units of work over SeaORM's \
     executor; with none, a unit runs bare.";

/// The driver's half of the port: one per app, bound as `Arc<dyn UnitOfWork>`
/// by the driver's binding, and the one trait a data-layer driver implements to
/// serve every edge.
pub trait UnitOfWork: Send + Sync + 'static {
    /// Open the unit `spec` declares — sending nothing to the database yet — or
    /// refuse it before its body runs, saying why (a policy the driver does not
    /// support).
    fn open(&self, spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured>;

    /// How long a unit's settle waits, at most, for the database to answer its
    /// commit or rollback: past it [`OpenUnit::settle`] answers
    /// [`Unhonoured::IN_DOUBT`] for a commit. Above every bound the database
    /// holds a statement to — SeaORM's is its statement budget plus a second —
    /// so it fires only on a database that stopped bounding itself.
    ///
    /// An edge whose own timer wraps a unit arms it this far past the unit's
    /// deadline: a net that never fires while the driver keeps its contract.
    fn settle_net(&self) -> Duration;
}

/// One unit a [`UnitOfWork`] opened, settled once by [`run_unit`].
///
/// A unit opened under [`TransactionPolicy::Pool`] settles nothing, and may be
/// dropped unsettled: a continuation that outlives its unit (a streaming body,
/// a DataLoader batch) runs on one. A unit holding a transaction that is
/// dropped unsettled is abandoned, and its driver rolls back and says so.
pub trait OpenUnit: Send {
    /// The handle installed as the ambient executor for the unit's body.
    fn executor(&self) -> Arc<dyn Executor>;

    /// Commit on `ok`, roll back otherwise, then run or drop the work held for
    /// the commit — and answer whether a success could be honoured.
    ///
    /// Called once the body and every handle the unit installed for it are
    /// dropped, so a handle still held is one that escaped the unit; one that
    /// settles nothing answers a ready future. What a driver owes:
    ///
    /// - **It completes once polled, even if the future is dropped**: the
    ///   driver runs it on a task of its own, which it drains when it closes,
    ///   so a client gone, a stopped operation or a deadline never cuts a
    ///   `COMMIT` mid-flight.
    /// - **It answers within [`settle_net`](UnitOfWork::settle_net)** of the
    ///   commit being sent, and [`Unhonoured::IN_DOUBT`] past it — a commit that
    ///   did not answer may have landed, so it is never retried.
    /// - **It waits for the work held for a commit until the unit's deadline**
    ///   ([`UnitSpec::deadline`]), then answers and lets that work run on: a
    ///   slow listener never turns a committed unit into a failure.
    ///
    /// [`run_unit`] waits for a failed unit's rollback until the deadline at
    /// most, since a failure stands whatever the driver answers.
    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>>;
}

/// How an [`OpenUnit`] settled.
///
/// It can grow within 7.x, so a `match` outside this crate keeps a `_` arm —
/// one that fails the unit, as [`run_unit`] does:
///
/// ```compile_fail,E0004
/// use nest_rs_database::Settlement;
///
/// fn kept(settlement: Settlement) -> bool {
///     match settlement {
///         Settlement::Settled => true,
///         Settlement::Unhonoured(_) => false,
///     }
/// }
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement {
    /// The unit's outcome stands: committed, rolled back, or nothing to settle.
    Settled,
    /// The unit succeeded and the driver could **not** honour it — nothing it
    /// wrote stands: it becomes the failure the [`Unhonoured`] describes.
    Unhonoured(Unhonoured),
}

/// The binding a unit was opened by, ambient for its body and for the
/// continuations that outlive it.
#[derive(Clone)]
pub(crate) struct Bound {
    pub(crate) unit_of_work: Arc<dyn UnitOfWork>,
    pub(crate) unit: Unit,
}

impl Bound {
    /// The pool a continuation of this unit runs on: a
    /// [`Pool`](TransactionPolicy::Pool) unit of the same binding, which settles
    /// nothing. `None` when the binding refuses one.
    pub(crate) fn pool(&self, scope: ExecutorScope) -> Option<Arc<dyn Executor>> {
        let spec = UnitSpec::new(self.unit, scope, TransactionPolicy::Pool);
        self.unit_of_work
            .open(&spec)
            .ok()
            .map(|open| open.executor())
    }
}

/// The edge's half of the port: run `body` as one unit of work and answer what
/// the edge reports.
///
/// With no binding (`None`) the body runs bare. Otherwise the binding opens the
/// unit `spec` declares — or refuses it, and `unhonoured` answers without
/// running the body — and `body` runs with the unit's executor installed under
/// the spec's scope, the binding ambient for [`current_unit_of_work`]. Once the
/// body ended and was dropped, the unit settles on `ok(&outcome)`: a unit the
/// driver could not honour turns a success into `unhonoured(why)`, and a
/// failure stands whatever the driver answers.
///
/// The edge cuts `body` at the spec's deadline itself, answering its timeout
/// in its own terms; `run_unit` never cuts a commit. A failure is answered by
/// the deadline at most, its rollback left to finish on the driver's task; a
/// success waits for its settle, which the driver bounds
/// ([`OpenUnit::settle`]).
///
/// An edge resolves its binding once, at mount or boot, and passes it here for
/// every unit.
///
/// ```
/// use nest_rs_core::operation_log::Unit;
/// use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitSpec, run_unit};
///
/// # fn spec(unit: Unit) -> UnitSpec {
/// #     UnitSpec::new(unit, ExecutorScope::Request, TransactionPolicy::PerUnit)
/// # }
/// # async fn handle(unit: Unit) -> Result<&'static str, &'static str> {
/// let answered = run_unit(
///     None,
///     spec(unit),
///     async { Ok("the handler's reply") },
///     Result::is_ok,
///     |_| Err("an internal error"),
/// )
/// .await;
/// # answered
/// # }
/// ```
pub fn run_unit<T, B>(
    unit: Option<&Arc<dyn UnitOfWork>>,
    spec: UnitSpec,
    body: B,
    ok: fn(&T) -> bool,
    unhonoured: fn(Unhonoured) -> T,
) -> impl Future<Output = T> + Send
where
    T: Send,
    B: Future<Output = T> + Send,
{
    // Not an `async fn`: its state would keep `body` beside the future built
    // from it, and `then` drops the body's future before the settle's is built.
    let Some(binding) = unit else {
        return Either::Left(Either::Left(body));
    };
    let open = match binding.open(&spec) {
        Ok(open) => open,
        Err(why) => return Either::Left(Either::Right(std::future::ready(unhonoured(why)))),
    };
    let bound = Bound {
        unit_of_work: Arc::clone(binding),
        unit: spec.unit,
    };
    let deadline = spec.deadline;
    let ran = install_unit(open.executor(), spec.scope, bound, body);
    Either::Right(ran.then(move |outcome| settle(open, outcome, ok, unhonoured, deadline)))
}

/// Settle `open` on `outcome`, which a success the driver could not honour
/// replaces.
async fn settle<T>(
    open: Box<dyn OpenUnit>,
    outcome: T,
    ok: fn(&T) -> bool,
    unhonoured: fn(Unhonoured) -> T,
    deadline: Option<tokio::time::Instant>,
) -> T {
    let succeeded = ok(&outcome);
    let settling = open.settle(succeeded);
    if !succeeded {
        // A rollback may wait on a statement the cut body left running: the
        // edge answers by its deadline, and the driver's task finishes it.
        match deadline {
            Some(deadline) => drop(tokio::time::timeout_at(deadline, settling).await),
            None => drop(settling.await),
        }
        return outcome;
    }
    match settling.await {
        Settlement::Settled => outcome,
        Settlement::Unhonoured(why) => unhonoured(why),
    }
}

/// The binding that opened the running unit, for in-process work that opens a
/// unit of its own without reaching the container; `None` outside any unit.
pub fn current_unit_of_work() -> Option<Arc<dyn UnitOfWork>> {
    crate::executor::current_bound().map(|bound| bound.unit_of_work)
}
