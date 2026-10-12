//! ORM-agnostic seam for the request/job data layer.
//!
//! `nest-rs-database` ships **only the seam**: the [`Executor`] trait, the
//! [`ExecutorScope`] tag, the installers and readers
//! ([`with_request_executor`], [`with_job_executor`], [`current_executor`],
//! [`current_executor_scope`]) that keep "the request's current handle on a
//! unit of work" in the kernel's ambient context, which carries it to a
//! streaming body, a DataLoader batch and after-commit work as the pool, and
//! to no task, so a detached write cannot run outside a unit of work —
//! [`after_commit`], the one way an effect waits for that unit of work to
//! commit, and [`atomic`](fn@atomic), a savepoint of that unit that rolls back alone. It
//! is what every ORM integration plugs into — not an ORM itself.
//! The non-HTTP side is wired through `nest_rs_worker::JobContext`, which a
//! worker transport (`#[scheduled]`, `#[processor]`) resolves before each job.
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
//! 5. Override [`Executor::atomic`] to offer [`atomic`](fn@atomic) a block: an
//!    [`AtomicBlock`] whose statements run inside a savepoint (a transaction of
//!    its own on a pool), settled once. The default offers none, and `atomic`
//!    then fails closed with [`AtomicError::Unsupported`].
//! 6. Match [`ExecutorScope`] with a `_` arm that fails closed: the enum can
//!    grow within 7.x.
//!
//! A continuation that can outlive the transaction — a streaming body, a
//! DataLoader batch, after-commit work — runs on the pool the first-party
//! driver hands out; another ORM's runs there with no executor until the
//! unit-of-work port lets it open one.
//!
//! A new ORM integration ships its own row-level-filter equivalent: the
//! SeaORM pieces (`Repo`, `condition_for`, `Bind<A, S>`) are unreachable from it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod atomic;
mod executor;

pub use atomic::{AtomicBlock, AtomicError, atomic};
pub use executor::{
    Deferred, Executor, ExecutorScope, after_commit, current_executor, current_executor_scope,
    with_executor, with_job_executor, with_request_executor,
};
