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
//! `Repo` (row-level filter), `CrudService`, the loader `bind = Service` runs,
//! and `SeaOrmDatabaseModule`, which binds its `UnitOfWork`.
//!
//! ## Extension contract
//!
//! To add a new ORM:
//!
//! 1. Implement [`Executor`] on what your units install, and say what each
//!    handle [holds](Executor::holds): the pool ([`HeldPool`]) or a
//!    transaction ([`HeldTransaction`]). The port reads the rest off that
//!    answer, and no method of the three traits has a default, so a handle
//!    holding a transaction cannot leave one out:
//!    - [`HeldTransaction::after_commit`] holds the work [`after_commit`]
//!      hands it, runs it after the commit — or after a unit that succeeded
//!      without opening one — and drops it otherwise; on a pool the port runs
//!      the work at once.
//!    - [`HeldTransaction::savepoint`] and [`HeldPool::transaction`] open the
//!      [`AtomicBlock`] [`atomic`](fn@atomic) runs its work in — a savepoint
//!      of the transaction, a transaction of its own on the pool — settled
//!      once. A resilience policy runs each attempt of a call it may send
//!      again in the savepoint, and on the pool itself.
//!    - [`HeldTransaction::outbox`] and [`HeldPool::outbox`] answer the
//!      outbox of step 5, or `None` for a driver keeping none.
//! 2. Implement [`UnitOfWork`]: `open` answers an [`OpenUnit`] for the
//!    [`UnitSpec`] an edge declares — a transaction opened on the unit's first
//!    touch for [`TransactionPolicy::PerUnit`], the pool for
//!    [`TransactionPolicy::Pool`] — whose `settle` commits or rolls back.
//!    Bind it once, through [`provide_unit_of_work`] from your module: every
//!    edge then opens its units through it, and every lifecycle hook runs in
//!    one.
//! 3. Provide your own `Repo`-equivalent query API that calls
//!    [`current_executor`] and recovers your handle off what it holds
//!    ([`Holds::downcast_ref`]) — a type private to your crate, so a service
//!    reading [`current_executor`] finds no handle it could run a statement on
//!    that your row filter never saw.
//! 4. Match [`ExecutorScope`] and [`TransactionPolicy`] with a `_` arm that
//!    fails closed: both can grow within 7.x.
//! 5. Keep the queue port's transactional outbox, so a job pushed inside a
//!    unit is filed after its commit and never on its rollback: implement
//!    [`OutboxStore`] over a table of your own, bind it as
//!    `Arc<dyn OutboxStore>` beside your unit of work and import the queue
//!    port's `OutboxModule`, which attaches the relay draining it; then answer
//!    an [`Outbox`] from every handle holding a transaction, writing on that
//!    transaction once the store is armed, and an [`OutboxKept`] from the pool
//!    handles your units install too, for a cancel there. A handle answering
//!    `None` has its pushes filed at once, and a rollback leaves the job
//!    behind.
//!
//! A new ORM integration ships its own row-level-filter equivalent: the
//! SeaORM pieces (`Repo`, `condition_for`, the bound-subject loader) are unreachable from it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod atomic;
mod error;
mod executor;
mod outbox;
mod unit;

pub use atomic::{AtomicBlock, atomic};
pub use error::{AtomicError, OutboxError, Unhonoured};
pub use executor::{
    Deferred, Executor, ExecutorScope, HeldPool, HeldTransaction, Holds, after_commit,
    current_executor, current_executor_scope,
};
pub use outbox::{ClaimedRecord, Outbox, OutboxClaim, OutboxKept, OutboxRecord, OutboxStore};
pub use unit::{
    BACKEND_REMEDY, JobRef, OpenUnit, Settlement, TransactionPolicy, UnitOfWork, UnitSpec,
    current_unit_of_work, provide_unit_of_work, run_unit,
};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's sibling crates. Not API: may change in any
    //! release.

    pub use crate::executor::{
        InstalledSavepoint, installed_scope, with_executor, with_job_executor,
        with_request_executor,
    };
    pub use crate::unit::run_unit_bounded;
}
