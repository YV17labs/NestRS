use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use features::authz::AuthzAbility;
use features::notifications::{Entity as Notifications, NotificationsQueueModule};
use features::posts::{
    ActiveModel as PostActive, Entity as Posts, PostStatus, PostsModule, PostsService,
};
use features::testing::{RedisDatabase, connection, seed_org, seed_user};
use features::users::UserRole;
use features::{Claims, Role, Subject};
use nest_rs::authz::{Ability, AbilityBuilder, AbilityFactory, with_ability};
use nest_rs::core::module;
use nest_rs::database::{
    ExecutorScope, Settlement, TransactionPolicy, UnitOfWork, UnitSpec, run_unit,
};
use nest_rs::queue::{OutboxRelay, QueueWorker};
use nest_rs::redis::RedisQueueModule;
use nest_rs::seaorm::SeaOrmDatabaseModule;
use nest_rs::testing::HeadlessApp;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

#[module(
    imports = [
        SeaOrmDatabaseModule,
        RedisQueueModule,
        PostsModule,
    ],
)]
struct PublishingHarness;

#[module(
    imports = [
        SeaOrmDatabaseModule,
        RedisQueueModule,
        NotificationsQueueModule,
    ],
)]
struct NotifyingWorkerHarness;

async fn author(conn: &DatabaseConnection) -> (Uuid, Uuid) {
    let org_id = seed_org(conn, "Acme").await;
    let author_id = seed_user(conn, org_id, "Ada", "ada@acme.test", UserRole::User).await;
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

fn publisher(org_id: Uuid, user: Uuid) -> Arc<Ability> {
    let claims = Claims {
        sub: Subject::User(user),
        client_id: "publish-web".into(),
        org_id,
        roles: vec![Role::Admin],
        scopes: Vec::new(),
    };
    let mut rules = AbilityBuilder::new();
    AuthzAbility.define(&claims, &mut rules);
    Arc::new(rules.build().expect("the rules build"))
}

async fn publish_in_an_attempt(
    unit_of_work: &Arc<dyn UnitOfWork>,
    svc: &PostsService,
    conn: &DatabaseConnection,
    id: Uuid,
    actor: Arc<Ability>,
    attempt_succeeds: bool,
) -> Settlement {
    let model = Posts::find_by_id(id)
        .one(conn)
        .await
        .expect("read the draft")
        .expect("the draft exists");
    let spec = UnitSpec::new(
        nest_rs::queue::unit::JOB,
        ExecutorScope::Job,
        TransactionPolicy::PerUnit,
    );
    let ran = run_unit(
        Some(unit_of_work),
        spec,
        async move {
            with_ability(actor, svc.publish(model))
                .await
                .expect("the post publishes inside the attempt");
            attempt_succeeds.ok_or(None)
        },
        Result::is_ok,
        |why| Err(Some(why)),
    )
    .await;
    match ran {
        Err(Some(why)) => Settlement::Unhonoured(why),
        Ok(()) | Err(None) => Settlement::Settled,
    }
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
    let app = HeadlessApp::builder()
        .module::<PublishingHarness>()
        .ephemeral_database::<migrations::Migrator>()
        .provide(RedisDatabase::PostsRollback.config())
        .build_headless()
        .await
        .expect("the publishing app boots against a throwaway database and Redis");
    let conn = connection(app.container());
    let (org_id, author_id) = author(&conn).await;
    let rolled_back = draft(&conn, org_id, author_id, "Rolled back").await;
    let committed = draft(&conn, org_id, author_id, "Committed").await;
    let relaying = app
        .spawn_transport(OutboxRelay::new())
        .await
        .expect("the relay arms the outbox over the throwaway database");
    app.init().await.expect("the init hooks run");
    let svc = app
        .container()
        .get::<PostsService>()
        .expect("PostsService is provided");
    let unit_of_work = app
        .container()
        .get_dyn::<dyn UnitOfWork>()
        .expect("SeaOrmDatabaseModule binds the unit of work");

    assert_eq!(
        publish_in_an_attempt(
            &unit_of_work,
            &svc,
            &conn,
            rolled_back,
            publisher(org_id, author_id),
            false,
        )
        .await,
        Settlement::Settled,
    );
    assert_eq!(
        publish_in_an_attempt(
            &unit_of_work,
            &svc,
            &conn,
            committed,
            publisher(org_id, author_id),
            true,
        )
        .await,
        Settlement::Settled,
    );
    assert_eq!(status(&conn, rolled_back).await, PostStatus::Draft);
    assert_eq!(status(&conn, committed).await, PostStatus::Published);

    let worker = HeadlessApp::builder()
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
    assert!(
        seen.iter().any(|message| message.contains("Committed")),
        "the committed publish's notification arrives within 20s: {seen:?}",
    );
    draining
        .shutdown()
        .await
        .expect("the worker's QueueWorker stops cleanly");
    relaying
        .shutdown()
        .await
        .expect("the publishing app's relay stops cleanly");

    assert_eq!(
        notifications(&conn).await,
        ["Post \"Committed\" was published"],
        "the queue is drained in order, one job at a time, so the committed publish's \
         notification lands after any job the rolled-back one could have left",
    );
}
