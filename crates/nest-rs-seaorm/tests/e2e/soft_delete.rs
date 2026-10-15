//! A soft-deleted row is hidden from every `Repo` read and update, and from
//! `CrudService::access`; `Repo::with_tombstones` reads it, `Repo::delete`
//! purges it, and a create may insert one. The entity names no service: the
//! declaration alone registers it.

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, with_ability};
use nest_rs_seaorm::{
    Access, Creatable, CrudService, Deletable, Executor, Repo, SoftDeletable, with_request_executor,
};
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Condition;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, IntoActiveModel, QueryFilter, Set,
};

mod gravestone {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Gravestone", soft_delete, timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "soft_delete_gravestone")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub name: String,
        #[expose]
        pub created_at: DateTimeWithTimeZone,
        #[expose]
        pub updated_at: DateTimeWithTimeZone,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }
}

struct GravestonesService;

impl CrudService for GravestonesService {
    type Entity = gravestone::Entity;
}

impl Creatable for GravestonesService {
    type Create = gravestone::CreateGravestone;
}

impl Deletable for GravestonesService {}

async fn seed(conn: &DatabaseConnection) -> gravestone::Model {
    crate::harness::setup_shared_table(
        conn,
        "soft_delete_gravestone",
        "CREATE TABLE IF NOT EXISTS soft_delete_gravestone ( \
             id uuid PRIMARY KEY, \
             name text NOT NULL, \
             created_at timestamptz NOT NULL, \
             updated_at timestamptz NOT NULL, \
             deleted_at timestamptz \
         );",
    )
    .await;
    gravestone::ActiveModel {
        id: Set(Uuid::now_v7()),
        name: Set("kept".to_owned()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed a live row")
}

fn manage() -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, gravestone::Entity);
    Arc::new(b.build().expect("valid test ability"))
}

fn this(id: Uuid) -> Condition {
    Condition::all().add(gravestone::Column::Id.eq(id))
}

#[tokio::test]
async fn a_soft_deleted_row_is_hidden_from_every_read_and_update_but_with_tombstones() {
    let conn = crate::harness::connect().await;
    let row = seed(&conn).await;
    let id = row.id;

    with_request_executor(Executor::Pool(conn.clone()), async {
        with_ability(manage(), async {
            GravestonesService
                .delete(row.clone())
                .await
                .expect("the entity's declaration makes the delete a soft one");
            let c = Repo::<gravestone::Entity>::conn().expect("an executor");

            assert!(
                Repo::<gravestone::Entity>::find_by_id(id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                !Repo::<gravestone::Entity>::all()
                    .await
                    .unwrap()
                    .iter()
                    .any(|r| r.id == id)
            );
            assert!(
                Repo::<gravestone::Entity>::scoped(Action::Read)
                    .filter(this(id))
                    .one(&c)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                Repo::<gravestone::Entity>::unscoped_by_id(id)
                    .one(&c)
                    .await
                    .unwrap()
                    .is_none()
            );
            let page = Repo::<gravestone::Entity>::page(100, None, this(id))
                .await
                .unwrap();
            assert!(page.items.is_empty(), "a page leaves it out");
            assert!(matches!(
                GravestonesService.access(Action::Read, id).await.unwrap(),
                Access::Missing
            ));

            let tombstone = Repo::<gravestone::Entity>::with_tombstones(Action::Read)
                .filter(this(id))
                .one(&c)
                .await
                .unwrap()
                .expect("the escape reads the tombstone");
            assert!(tombstone.deleted_at.is_some(), "stamped, not erased");

            let mut renamed = tombstone.clone().into_active_model();
            renamed.name = Set("revived".to_owned());
            assert!(matches!(
                Repo::<gravestone::Entity>::update(renamed).await,
                Err(DbErr::RecordNotUpdated)
            ));

            assert!(
                !Repo::<gravestone::Entity>::can(Action::Read, &tombstone)
                    .await
                    .unwrap()
            );
            assert!(
                !Repo::<gravestone::Entity>::can(Action::Update, &tombstone)
                    .await
                    .unwrap()
            );
            assert!(
                Repo::<gravestone::Entity>::can(Action::Delete, &tombstone)
                    .await
                    .unwrap(),
                "the hard purge reaches a tombstone",
            );

            Repo::<gravestone::Entity>::soft_delete(
                tombstone.clone(),
                gravestone::Entity::deleted_at_column(),
            )
            .await
            .expect("a second soft delete is idempotent");

            let purged = Repo::<gravestone::Entity>::delete(tombstone).await.unwrap();
            assert_eq!(purged.rows_affected, 1, "the hard purge erases it");
            assert!(
                Repo::<gravestone::Entity>::with_tombstones(Action::Read)
                    .filter(this(id))
                    .one(&c)
                    .await
                    .unwrap()
                    .is_none()
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn a_row_inserted_already_deleted_passes_the_create_check_and_reads_as_absent() {
    let conn = crate::harness::connect().await;
    seed(&conn).await;
    let id = Uuid::now_v7();

    with_request_executor(Executor::Pool(conn.clone()), async {
        with_ability(manage(), async {
            let archived = gravestone::ActiveModel {
                id: Set(id),
                name: Set("archived".to_owned()),
                deleted_at: Set(Some(chrono::Utc::now().fixed_offset())),
                ..Default::default()
            };
            let row = GravestonesService
                .create_from_active(archived)
                .await
                .expect("the create check tests the ability alone, not the live condition");
            assert!(row.deleted_at.is_some(), "inserted as deleted");
            assert!(
                Repo::<gravestone::Entity>::find_by_id(id)
                    .await
                    .unwrap()
                    .is_none(),
                "an imported tombstone reads as absent",
            );
        })
        .await;
    })
    .await;
}
