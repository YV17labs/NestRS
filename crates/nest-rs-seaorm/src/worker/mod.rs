//! Worker bridge — the [`JobContext`](nest_rs_worker::JobContext)
//! implementation that installs the pool executor around each queue or
//! schedule job. Auto-bound by [`DatabaseModule`](crate::DatabaseModule).

mod context;

pub use context::WorkerDbContext;
