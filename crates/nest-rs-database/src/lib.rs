//! ORM-agnostic seam for the request/job data layer.
//!
//! `nest-rs-database` ships **only the seam**: the [`Executor`] trait, the
//! [`ExecutorScope`] tag, and the `tokio::task_local!` plumbing
//! ([`with_request_executor`], [`with_job_executor`], [`current_executor`],
//! [`current_executor_scope`]) that carries "the request's current handle
//! on a unit of work" across the framework. It is what every ORM
//! integration plugs into — not an ORM itself. The non-HTTP side is wired
//! through `nest_rs_worker::JobContext`, which a worker transport
//! (`#[scheduled]`, `#[processor]`) resolves before each job.
//!
//! The first-class implementation is `nest-rs-seaorm` (SeaORM): it ships
//! `Repo` (row-level filter), `CrudService`, `Bind`, the HTTP mask
//! shaper, and `DatabaseModule` (the request interceptor that opens the
//! transaction). Those pieces are SeaORM-specific by design — the
//! leverage comes from binding tightly to the ORM's query/model types.
//! A future third-party `nest-rs-<other-orm>` crate (sqlx, diesel,
//! prisma-client-rust, mongo, …) can plug a different engine into the
//! same ambient seam without touching `nest-rs-core` or any feature code.
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
//!
//! The SeaORM-specific pieces (`Repo`, `condition_for`, the mask shaper,
//! `Bind<S, A>`, `CrudService`) are unreachable from your implementation —
//! that is intentional. They couple to SeaORM's `EntityTrait`/`Model`; a
//! generic abstraction over them would lose 80% of their value. A new ORM
//! integration ships its own row-level-filter equivalent.
mod executor;

pub use executor::{
    Executor, ExecutorScope, current_executor, current_executor_scope, with_executor,
    with_job_executor, with_request_executor,
};
