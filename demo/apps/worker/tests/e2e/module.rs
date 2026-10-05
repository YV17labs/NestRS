use std::time::Duration;

use features::notifications::{Column, Entity, NotifyCommand, NotifyQueue};
use features::testing::RedisDatabase;
use nest_rs::core::module;
use nest_rs::queue::JobProducerExt;
use nest_rs::queue::QueueWorker;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisQueueProducer};
use nest_rs::schedule::Scheduler;
use nest_rs::testing::{EphemeralDatabase, HeadlessApp, TestApp};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use uuid::Uuid;
use worker::WorkerModule;

#[module(imports = [RedisModule::for_root(None), RedisQueueModule])]
struct SuitesProducer;

async fn notify(app: &HeadlessApp, org_id: Uuid) {
    app.container()
        .get::<RedisQueueProducer>()
        .expect("RedisQueueModule bound the producer")
        .push(
            NotifyQueue,
            NotifyCommand {
                org_id,
                message: format!("for {org_id}"),
            },
            None,
        )
        .await
        .expect("enqueue a notification");
}

async fn notifications(conn: &DatabaseConnection, org_id: Uuid) -> u64 {
    Entity::find()
        .filter(Column::OrgId.eq(org_id))
        .count(conn)
        .await
        .expect("count the org's notifications")
}

#[tokio::test]
async fn the_worker_app_runs_the_jobs_on_its_own_database_and_none_from_the_suites() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let worker = TestApp::builder()
        .module::<WorkerModule>()
        .provide_arc(db.connection())
        .provide(RedisDatabase::WorkerRuns.config())
        .build_headless()
        .await
        .expect("WorkerModule boots against the throwaway database and its own Redis database");
    let queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("WorkerModule's QueueWorker configures against Redis");

    let suites = TestApp::builder()
        .module::<SuitesProducer>()
        .build_headless()
        .await
        .expect("a producer boots on the suites' Redis database");
    let foreign = Uuid::now_v7();
    notify(&suites, foreign).await;
    let ours = Uuid::now_v7();
    notify(&worker, ours).await;

    let conn = db.connection();
    let mut ran = false;
    for _ in 0..60 {
        if notifications(&conn, ours).await == 1 {
            ran = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stolen = notifications(&conn, foreign).await;

    queue
        .shutdown()
        .await
        .expect("WorkerModule's QueueWorker stops cleanly");

    assert!(ran, "the worker ran the job enqueued on its own database");
    assert_eq!(
        stolen, 0,
        "the worker ran a job enqueued on the suites' database"
    );
}

#[tokio::test]
async fn worker_app_binds_the_lock_its_one_replica_purge_claims_through() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let worker = TestApp::builder()
        .module::<WorkerModule>()
        .provide_arc(db.connection())
        .build_headless()
        .await
        .expect("WorkerModule boots against the throwaway database and Redis");
    let scheduler = worker
        .spawn_transport(Scheduler::new())
        .await
        .expect("the scheduler configures: the purge declared replicas = \"one\" has a lock");
    scheduler
        .shutdown()
        .await
        .expect("the scheduler stops cleanly");
}
