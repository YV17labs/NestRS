use std::sync::Arc;

use chrono::{TimeDelta, Utc};
use features::notifications::{ActiveModel, Column, Entity, NotificationsService};
use nest_rs::authz::AbilityBuilder;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::{connection, data_app};

async fn seed_notification(conn: &DatabaseConnection, age: TimeDelta) -> Uuid {
    let id = Uuid::now_v7();
    ActiveModel {
        id: Set(id),
        org_id: Set(Uuid::now_v7()),
        message: Set("Post \"Seeded\" was published".to_owned()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed notification");
    let backdated = Entity::update_many()
        .col_expr(
            Column::CreatedAt,
            Expr::value((Utc::now() - age).fixed_offset()),
        )
        .filter(Column::Id.eq(id))
        .exec(conn)
        .await
        .expect("backdate the notification");
    assert_eq!(backdated.rows_affected, 1);
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
    let app = data_app().await;
    let conn = connection(&app);
    let expired = seed_notification(conn.as_ref(), TimeDelta::days(31)).await;
    let kept = seed_notification(conn.as_ref(), TimeDelta::days(29)).await;

    let purged = app
        .system_unit(|| NotificationsService.purge_expired())
        .await
        .expect("the purge runs as system work");

    assert_eq!(purged, 1);
    assert!(!exists(conn.as_ref(), expired).await);
    assert!(exists(conn.as_ref(), kept).await);
}

#[tokio::test]
async fn the_purge_deletes_nothing_for_a_request_granted_nothing() {
    let app = data_app().await;
    let conn = connection(&app);
    let expired = seed_notification(conn.as_ref(), TimeDelta::days(31)).await;

    let nothing = Arc::new(
        AbilityBuilder::new()
            .build()
            .expect("an empty ability builds"),
    );
    let purged = app
        .unit(nothing, || NotificationsService.purge_expired())
        .await
        .expect("a denied purge is not an error");

    assert_eq!(purged, 0);
    assert!(exists(conn.as_ref(), expired).await);
}
