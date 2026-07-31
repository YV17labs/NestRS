//! B11: `Repo::update` must run `ActiveModelBehavior`.
//!
//! The scope filter forces the sea-orm query-builder path (`Update::one`),
//! which does not call the behaviour hooks — while the create path does, via
//! `ActiveModelTrait::insert`. The asymmetry meant the `timestamps` flag
//! stamped `created_at` on insert and **never moved `updated_at`**: the PATCH
//! succeeded, the row changed, and the column downstream caches, incremental
//! sync and ETags trust stayed byte-identical to `created_at` forever.
//!
//! Asserted against live Postgres on the primitive itself, so it holds for
//! every behaviour a resource declares, not only the macro-emitted one.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action, with_ability};
use nest_rs_seaorm::{Executor, Repo, with_request_executor};
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait, Set};

mod stamped {
    use sea_orm::entity::prelude::*;
    use sea_orm::{ActiveValue, DbErr};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "lifecycle_probe_stamped")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub name: String,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    /// The shape `#[expose(..., timestamps)]` emits: `created_at` on insert,
    /// `updated_at` on **every** save.
    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C>(mut self, _db: &C, insert: bool) -> Result<Self, DbErr>
        where
            C: ConnectionTrait,
        {
            let now: DateTimeWithTimeZone = chrono::Utc::now().fixed_offset();
            if insert {
                self.created_at = ActiveValue::Set(now);
            }
            self.updated_at = ActiveValue::Set(now);
            Ok(self)
        }
    }
}

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "lifecycle_probe_stamped",
        "CREATE TABLE IF NOT EXISTS lifecycle_probe_stamped (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        );",
    )
    .await;
    conn
}

fn allow_everything() -> Arc<nest_rs_authz::Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, stamped::Entity);
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn repo_update_runs_active_model_behavior_so_updated_at_moves() {
    let conn = db().await;
    let id = Uuid::now_v7();
    let epoch: DateTimeWithTimeZone = chrono::Utc::now().fixed_offset();

    // The create path already runs the hook (`ActiveModelTrait::insert`), so
    // both stamps land at insert time and are equal — exactly the state the
    // bug froze forever.
    let inserted = stamped::ActiveModel {
        id: Set(id),
        name: Set("before".to_owned()),
        created_at: Set(epoch),
        updated_at: Set(epoch),
    }
    .insert(&conn)
    .await
    .expect("seed row inserts");
    assert_eq!(
        inserted.created_at, inserted.updated_at,
        "the insert stamps both columns at once",
    );

    // Timestamps are microsecond-precision; leave room for a strict `>`.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    with_request_executor(Executor::Pool(conn.clone()), async {
        with_ability(allow_everything(), async {
            let updated = Repo::<stamped::Entity>::update(stamped::ActiveModel {
                id: ActiveValue::Unchanged(id),
                name: Set("after".to_owned()),
                ..Default::default()
            })
            .await
            .expect("the scoped update executes");

            assert_eq!(updated.name, "after", "the update still applies");
            assert_eq!(
                updated.created_at, inserted.created_at,
                "`created_at` is insert-only and must not move",
            );
            assert!(
                updated.updated_at > inserted.updated_at,
                "`updated_at` must be bumped by the behaviour hook — got {} (inserted {})",
                updated.updated_at,
                inserted.updated_at,
            );
        })
        .await;
    })
    .await;

    // …and the movement is in the database, not only in the returned model.
    let row = stamped::Entity::find_by_id(id)
        .one(&conn)
        .await
        .expect("read back")
        .expect("row present");
    assert!(row.updated_at > row.created_at, "the stored row moved too");

    stamped::Entity::delete_by_id(id)
        .exec(&conn)
        .await
        .expect("cleanup");
}
