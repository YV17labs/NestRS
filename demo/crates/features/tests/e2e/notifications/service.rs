use chrono::{TimeDelta, Utc};
use features::notifications::{ActiveModel, Entity, NotificationsService};
use nest_rs::seaorm::{Executor, with_job_executor, with_request_executor};
use nest_rs::testing::EphemeralDatabase;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

async fn seed_notification(conn: &DatabaseConnection, age: TimeDelta) -> Uuid {
    let id = Uuid::now_v7();
    ActiveModel {
        id: Set(id),
        org_id: Set(Uuid::now_v7()),
        message: Set("Post \"Seeded\" was published".to_owned()),
        created_at: Set((Utc::now() - age).fixed_offset()),
    }
    .insert(conn)
    .await
    .expect("seed notification");
    id
}

async fn exists(conn: &DatabaseConnection, id: Uuid) -> bool {
    Entity::find_by_id(id)
        .one(conn)
        .await
        .expect("read the notification back")
        .is_some()
}

#[tokio::test]
async fn the_purge_deletes_what_is_past_retention_and_keeps_the_rest() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let expired = seed_notification(conn.as_ref(), TimeDelta::days(31)).await;
    let kept = seed_notification(conn.as_ref(), TimeDelta::days(29)).await;

    let purged = with_job_executor(
        Executor::Pool((*conn).clone()),
        NotificationsService.purge_expired(),
    )
    .await
    .expect("the purge runs as system work");

    assert_eq!(purged, 1);
    assert!(!exists(conn.as_ref(), expired).await);
    assert!(exists(conn.as_ref(), kept).await);
}

#[tokio::test]
async fn the_purge_deletes_nothing_for_a_request_without_an_ability() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let expired = seed_notification(conn.as_ref(), TimeDelta::days(31)).await;

    let purged = with_request_executor(
        Executor::Pool((*conn).clone()),
        NotificationsService.purge_expired(),
    )
    .await
    .expect("a denied purge is not an error");

    assert_eq!(purged, 0);
    assert!(exists(conn.as_ref(), expired).await);
}
