//! Worker bridge — the [`JobContext`](nest_rs_worker::JobContext)
//! implementation that installs the job executor around each queue or
//! schedule job. Auto-bound by [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule).

mod context;

pub use context::WorkerDbContext;
