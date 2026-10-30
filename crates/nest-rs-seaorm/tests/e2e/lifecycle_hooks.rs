//! What `#[expose]` puts on an insert and a save, against Postgres: `Repo`'s
//! inserts fill the column defaults the entity registered, beside a hook of
//! its own; and `Repo::update` runs the `ActiveModelBehavior`
//! `#[expose(..., timestamps)]` emits, though the scope filter takes sea-orm's
//! `Update::one` path, which skips the behaviour hooks.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action, with_ability};
use nest_rs_database::{with_job_executor, with_request_executor};
use nest_rs_seaorm::{Creatable, CrudService, Repo, SeaOrmExecutor};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait, Set};

mod stamped {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Stamped", timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "lifecycle_probe_stamped")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        #[expose]
        pub created_at: DateTimeWithTimeZone,
        #[expose]
        pub updated_at: DateTimeWithTimeZone,
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

    let inserted = stamped::ActiveModel {
        id: Set(id),
        name: Set("before".to_owned()),
        ..Default::default()
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

    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
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

mod hooked {
    use nest_rs_seaorm::expose;
    use sea_orm::ActiveValue;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Hooked")]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "lifecycle_probe_hooked")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub name: String,
        #[expose(default = String::from("draft"))]
        pub stage: String,
    }

    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C>(mut self, _db: &C, _insert: bool) -> Result<Self, DbErr>
        where
            C: ConnectionTrait,
        {
            if let ActiveValue::Set(name) = &self.name {
                self.name = ActiveValue::Set(name.trim().to_owned());
            }
            Ok(self)
        }
    }
}

struct HookedService;

impl CrudService for HookedService {
    type Entity = hooked::Entity;
}

impl Creatable for HookedService {
    type Create = hooked::CreateHooked;
}

/// No column default in the table: a key or a stage the insert left unset
/// would fail its NOT NULL.
async fn hooked_db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "lifecycle_probe_hooked",
        "CREATE TABLE IF NOT EXISTS lifecycle_probe_hooked (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            stage TEXT NOT NULL
        );",
    )
    .await;
    conn
}

#[tokio::test]
async fn an_unscoped_insert_fills_the_column_defaults_and_runs_the_entitys_own_hook() {
    let conn = hooked_db().await;

    let inserted = with_job_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        Repo::<hooked::Entity>::insert_unscoped(hooked::ActiveModel {
            name: Set("  unscoped  ".to_owned()),
            ..Default::default()
        })
        .await
    })
    .await
    .expect("the defaults fill the key and the stage");

    assert_eq!(inserted.id.get_version_num(), 7);
    assert_eq!(inserted.stage, "draft");
    assert_eq!(inserted.name, "unscoped", "the entity's own hook ran");
    let row = hooked::Entity::find_by_id(inserted.id)
        .one(&conn)
        .await
        .expect("read back")
        .expect("the row persisted");
    assert_eq!(row, inserted);

    hooked::Entity::delete_by_id(inserted.id)
        .exec(&conn)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn a_create_fills_the_column_defaults_its_input_does_not_carry() {
    let conn = hooked_db().await;
    let mut b = AbilityBuilder::new();
    b.can(Action::Create, hooked::Entity);

    let created = with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        with_ability(Arc::new(b.build().expect("a valid ability")), async {
            HookedService
                .create(hooked::CreateHooked {
                    name: "  created  ".to_owned(),
                })
                .await
        }),
    )
    .await
    .expect("an in-scope create succeeds");

    assert_eq!(created.stage, "draft");
    assert_eq!(created.name, "created", "the entity's own hook ran");

    hooked::Entity::delete_by_id(created.id)
        .exec(&conn)
        .await
        .expect("cleanup");
}
