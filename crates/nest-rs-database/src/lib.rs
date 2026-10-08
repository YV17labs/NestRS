//! ORM-agnostic seam for the request/job data layer.
//!
//! `nest-rs-database` ships **only the seam**: the [`Executor`] trait, the
//! [`ExecutorScope`] tag, the `tokio::task_local!` plumbing
//! ([`with_request_executor`], [`with_job_executor`], [`current_executor`],
//! [`current_executor_scope`]) that carries "the request's current handle
//! on a unit of work" across the framework, and [`after_commit`] — the one
//! way an effect waits for that unit of work to commit. It is what every ORM
//! integration plugs into — not an ORM itself. The non-HTTP side is wired
//! through `nest_rs_worker::JobContext`, which a worker transport
//! (`#[scheduled]`, `#[processor]`) resolves before each job.
//!
//! The first-class implementation is `nest-rs-seaorm` (SeaORM): it ships
//! `Repo` (row-level filter), `CrudService`, `Bind`, the HTTP mask
//! shaper, and `SeaOrmDatabaseModule` (the request interceptor that opens the
//! transaction).
//!
//! ## Extension contract
//!
//! To add a new ORM:
//!
//! 1. Implement [`Executor`] on the type that represents your handle (a
//!    pool, a transaction, or an enum forwarding to either).
//! 2. Ship a `Module` that, for each HTTP request, wraps the handler in
//!    [`with_request_executor`] passing your `Arc<dyn Executor>`. For
//!    worker transports do the same with [`with_job_executor`] via
//!    `nest_rs_worker::JobContext`.
//! 3. Provide your own `Repo`-equivalent query API that calls
//!    [`current_executor`] and downcasts to your concrete type.
//! 4. Override [`Executor::after_commit`] on every handle a boundary settles a
//!    transaction for: hold the work, run it after the commit — or after a
//!    boundary that succeeded without opening one — and drop it otherwise. The
//!    default runs it at once, which is wrong for a transaction.
//!
//! A new ORM integration ships its own row-level-filter equivalent: the
//! SeaORM pieces (`Repo`, `condition_for`, `Bind<A, S>`) are unreachable from it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod executor;

pub use executor::{
    Deferred, Executor, ExecutorScope, after_commit, current_executor, current_executor_scope,
    with_executor, with_job_executor, with_request_executor,
};
