//! ORM-agnostic port for the request/job data layer: the unit of work every
//! edge runs through, and the ambient executor its body reads.
//!
//! `nest-rs-database` ships **only the port**. An edge runs each unit of work
//! through [`run_unit`], which asks the bound [`UnitOfWork`] to open it, runs
//! the body with the unit's [`Executor`] installed in the kernel's ambient
//! context, and settles it on the edge's own outcome. The readers
//! ([`current_executor`], [`current_executor_scope`], [`current_unit_of_work`])
//! serve the code the body calls; a streaming body, a DataLoader batch and
//! after-commit work run on a pool unit of the same binding, and a task gets
//! none, so a detached write cannot run outside a unit of work.
//! [`after_commit`] is the one way an effect waits for that unit of work to
//! commit, and [`atomic`](fn@atomic) a savepoint of it that rolls back alone.
//! It is what every ORM integration plugs into — not an ORM itself.
//!
//! The first-class implementation is `nest-rs-seaorm` (SeaORM): it ships
//! `Repo` (row-level filter), `CrudService`, `Bind`, the HTTP mask shaper,
//! and `SeaOrmDatabaseModule`, which binds its `UnitOfWork`.
//!
//! ## Extension contract
//!
//! To add a new ORM:
//!
//! 1. Implement [`Executor`] on the type that represents your handle (a
//!    pool, a transaction, or an enum forwarding to either).
//! 2. Implement [`UnitOfWork`]: `open` answers an [`OpenUnit`] for the
//!    [`UnitSpec`] an edge declares — a transaction opened on the unit's first
//!    touch for [`TransactionPolicy::PerUnit`], the pool for
//!    [`TransactionPolicy::Pool`] — whose `settle` commits or rolls back.
//!    Bind it once, through [`provide_unit_of_work`] from your module: every
//!    edge then opens its units through it, and every lifecycle hook runs in
//!    one.
//! 3. Provide your own `Repo`-equivalent query API that calls
//!    [`current_executor`] and downcasts to your concrete type.
//! 4. Override [`Executor::after_commit`] on every handle a unit settles a
//!    transaction for: hold the work, run it after the commit — or after a
//!    unit that succeeded without opening one — and drop it otherwise. The
//!    default runs it at once, which is wrong for a transaction.
//! 5. Override [`Executor::atomic`] to offer [`atomic`](fn@atomic) a block: an
//!    [`AtomicBlock`] whose statements run inside a savepoint (a transaction of
//!    its own on a pool), settled once. The default offers none, and `atomic`
//!    then fails closed with [`AtomicError::Unsupported`].
//! 6. Match [`ExecutorScope`] and [`TransactionPolicy`] with a `_` arm that
//!    fails closed: both can grow within 7.x.
//!
//! A new ORM integration ships its own row-level-filter equivalent: the
//! SeaORM pieces (`Repo`, `condition_for`, `Bind<A, S>`) are unreachable from it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod atomic;
mod error;
mod executor;
mod unit;

pub use atomic::{AtomicBlock, atomic};
pub use error::{AtomicError, Unhonoured};
pub use executor::{
    Deferred, Executor, ExecutorScope, after_commit, current_executor, current_executor_scope,
    with_executor, with_job_executor, with_request_executor,
};
pub use unit::{
    BACKEND_REMEDY, JobRef, OpenUnit, Settlement, TransactionPolicy, UnitOfWork, UnitSpec,
    current_unit_of_work, provide_unit_of_work, run_unit,
};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's sibling crates. Not API: may change in any
    //! release.

    pub use crate::unit::run_unit_bounded;
}
