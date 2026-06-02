//! [`WorkerDbContext`] — the ORM bridge for a worker transport's
//! [`JobContext`](nestrs_core::JobContext) seam, the cron/queue counterpart of the
//! HTTP [`DbContext`](crate::DbContext) interceptor.
//!
//! It installs a pool [`Executor`] around a scheduled tick or queue job, so the
//! job's [`Repo`](crate::Repo) calls join a connection without injecting one.
//! [`DatabaseModule`](crate::DatabaseModule) auto-binds it (like `DbContext` for
//! HTTP), so importing the module activates ambient `Repo` for jobs exactly as it
//! does for requests — no app wiring.
//!
//! A job runs on the connection **pool**, never a transaction — like a WebSocket
//! message and for the same reason: a worker job has no safe/mutating HTTP method
//! to classify into a per-job transaction (that remains a deliberate follow-up).
//! With no caller there is also no ambient ability, so a job's `Repo` reads/writes
//! are unscoped (the SQL identity filter) — correct for system work with no
//! principal to scope to.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_core::JobContext;
use sea_orm::DatabaseConnection;

use crate::executor::{with_job_executor, Executor};

/// Installs the request-less pool executor around a worker job. An `#[injectable]`
/// so it is built from the container (resolving the connection) exactly like any
/// provider; bound to `dyn JobContext` by [`DatabaseModule`](crate::DatabaseModule).
#[injectable]
pub struct WorkerDbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl JobContext for WorkerDbContext {
    fn scope<'a>(
        &'a self,
        inner: Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(with_job_executor(Executor::Pool(self.db.clone()), inner))
    }
}
