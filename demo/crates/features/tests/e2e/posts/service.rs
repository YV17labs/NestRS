use std::time::Duration;

use chrono::Utc;
use features::notifications::{
    Entity as Notifications, NotificationsEventsModule, NotificationsQueueModule,
};
use features::orgs::ActiveModel as OrgActive;
use features::posts::{
    ActiveModel as PostActive, Entity as Posts, PostStatus, PostsModule, PostsService,
};
use features::testing::RedisDatabase;
use features::users::{ActiveModel as UserActive, UserRole};
use nest_rs::core::module;
use nest_rs::queue::{QueueModule, QueueWorker};
use nest_rs::redis::{RedisModule, RedisQueueModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::testing::{EphemeralDatabase, TestApp};
use nest_rs::worker::{JobContext, JobSettlement, JobTransaction};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

#[module(
    imports = [
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        PostsModule,
        NotificationsEventsModule,
    ],
)]
struct PublishingHarness;

#[module(
    imports = [
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        QueueModule::for_root(None),
        NotificationsQueueModule,
    ],
)]
struct NotifyingWorkerHarness;

async fn author(conn: &DatabaseConnection) -> (Uuid, Uuid) {
    let org_id = Uuid::now_v7();
    let author_id = Uuid::now_v7();
    OrgActive {
        id: Set(org_id),
        name: Set("Acme".into()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");
    UserActive {
        id: Set(author_id),
        org_id: Set(org_id),
        name: Set("Ada".into()),
        email: Set("ada@acme.test".into()),
        role: Set(UserRole::User),
        password_hash: Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed author");
    (org_id, author_id)
}

async fn draft(conn: &DatabaseConnection, org_id: Uuid, author_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    PostActive {
        id: Set(id),
        org_id: Set(org_id),
        author_id: Set(author_id),
        title: Set(title.into()),
        body: Set("Body".into()),
        status: Set(PostStatus::Draft),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed a draft");
    id
}

async fn status(conn: &DatabaseConnection, id: Uuid) -> PostStatus {
    Posts::find_by_id(id)
        .one(conn)
        .await
        .expect("read the post")
        .expect("the post exists")
        .status
}

async fn publish_in_an_attempt(
    context: &dyn JobContext,
    svc: &PostsService,
    conn: &DatabaseConnection,
    id: Uuid,
    actor_id: Uuid,
    attempt_succeeds: bool,
) -> JobSettlement {
    let model = Posts::find_by_id(id)
        .one(conn)
        .await
        .expect("read the draft")
        .expect("the draft exists");
    context
        .scope(
            JobTransaction::PerAttempt,
            Box::pin(async move {
                svc.publish(model, actor_id)
                    .await
                    .expect("the post publishes inside the attempt");
                attempt_succeeds
            }),
        )
        .await
}

async fn notifications(conn: &DatabaseConnection) -> Vec<String> {
    Notifications::find()
        .all(conn)
        .await
        .expect("read the notifications")
        .into_iter()
        .map(|notification| notification.message)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publish_whose_transaction_rolls_back_enqueues_no_notification() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let conn = db.connection();
    let (org_id, author_id) = author(&conn).await;
    let rolled_back = draft(&conn, org_id, author_id, "Rolled back").await;
    let committed = draft(&conn, org_id, author_id, "Committed").await;

    let app = TestApp::builder()
        .module::<PublishingHarness>()
        .provide_arc(conn.clone())
        .provide(RedisDatabase::PostsRollback.config())
        .build_headless()
        .await
        .expect("the publishing app boots against the throwaway database and Redis");
    app.init().await.expect("the event listeners are wired");
    let svc = app
        .container()
        .get::<PostsService>()
        .expect("PostsService is provided");
    let context = app
        .container()
        .get_dyn::<dyn JobContext>()
        .expect("SeaOrmDatabaseModule binds the job context");

    assert_eq!(
        publish_in_an_attempt(&*context, &svc, &conn, rolled_back, author_id, false).await,
        JobSettlement::Settled,
    );
    assert_eq!(
        publish_in_an_attempt(&*context, &svc, &conn, committed, author_id, true).await,
        JobSettlement::Settled,
    );
    assert_eq!(status(&conn, rolled_back).await, PostStatus::Draft);
    assert_eq!(status(&conn, committed).await, PostStatus::Published);

    let worker = TestApp::builder()
        .module::<NotifyingWorkerHarness>()
        .provide_arc(conn.clone())
        .provide(RedisDatabase::PostsRollback.config())
        .build_headless()
        .await
        .expect("the notifications worker boots against the same database and Redis");
    let draining = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the QueueWorker drains the notifications queue");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen = notifications(&conn).await;
    while !seen.iter().any(|message| message.contains("Committed"))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
        seen = notifications(&conn).await;
    }
    draining
        .shutdown()
        .await
        .expect("the worker's QueueWorker stops cleanly");

    assert_eq!(
        notifications(&conn).await,
        ["Post \"Committed\" was published"],
        "the queue is drained in order, one job at a time, so the committed publish's \
         notification lands after any job the rolled-back one could have left",
    );
}
