//! `WorkerDbContext` installs a live pool executor around a job so a
//! `#[scheduled]`/`#[processor]` queries through `Repo` without an injected
//! connection. Driven against the dev Postgres.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_seaorm::{Executor, WorkerDbContext, current_executor};
use nest_rs_worker::JobContext;
use sea_orm::ConnectionTrait;

#[tokio::test]
async fn worker_db_context_installs_a_live_pool_executor_for_a_job() {
    let conn = crate::harness::connect_arc().await;

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
