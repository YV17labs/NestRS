//! [`WorkerDbContext`] — ORM bridge for worker transports' [`JobContext`]
//! seam, the cron/queue counterpart of [`DbContext`](crate::DbContext).
//! Auto-bound by [`DatabaseModule`](crate::DatabaseModule).
//!
//! A job runs on the **pool**, never a transaction (no HTTP method to classify).
//! No caller ⇒ no ambient ability ⇒ `Repo` reads/writes are unscoped — correct
//! for system work with no principal to scope to.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_worker::JobContext;
use sea_orm::DatabaseConnection;

use crate::executor::{Executor, with_job_executor};

/// Installs the request-less pool executor around a worker job. Bound to
/// `dyn JobContext` by [`DatabaseModule`](crate::DatabaseModule).
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
        Box::pin(with_job_executor(Executor::Pool((*self.db).clone()), inner))
    }
}
