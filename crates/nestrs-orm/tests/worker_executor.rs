//! `WorkerDbContext` — the worker bridge for the `JobContext` seam — installs a
//! live **pool** executor around a job, so a `#[cron_job]`/`#[processor]` queries
//! through `Repo` without injecting a connection. Driven against the dev Postgres:
//! the executor is absent outside the job, a live pool inside it, and unwinds after.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nestrs_core::{Container, JobContext};
use nestrs_orm::{current_executor, Executor, WorkerDbContext};
use sea_orm::{ConnectionTrait, Database};

#[tokio::test]
async fn worker_db_context_installs_a_live_pool_executor_for_a_job() {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at a reachable Postgres for this test");
    let conn = Arc::new(Database::connect(&url).await.expect("connect to Postgres"));

    // Build the bridge the way `DatabaseModule` does — from the container.
    let container = Container::builder().provide_arc(conn).build();
    let ctx: Arc<dyn JobContext> = Arc::new(WorkerDbContext::from_container(&container));

    assert!(
        current_executor().is_none(),
        "no ambient executor exists outside a job",
    );

    let job: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(async {
        let executor = current_executor().expect("the job runs with an ambient executor installed");
        assert!(
            matches!(executor, Executor::Pool(_)),
            "a worker job runs on the connection pool, not a per-job transaction",
        );
        // The executor is live, not a placeholder: a real statement runs through it,
        // so a job's `Repo` query (which reads this same ambient executor) would too.
        executor
            .execute_unprepared("SELECT 1")
            .await
            .expect("a query runs through the installed pool executor");
    });
    ctx.scope(job).await;

    assert!(
        current_executor().is_none(),
        "the ambient executor unwinds once the job completes",
    );
}
