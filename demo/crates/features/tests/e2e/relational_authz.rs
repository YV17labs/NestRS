use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, with_ability};
use nest_rs_seaorm::{
    Access, Creatable, CreateModel, CrudService, Executor, with_request_executor,
};
use nest_rs_testing::EphemeralDatabase;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbErr, Set};
use uuid::Uuid;

mod container {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "rel_container")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod item {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "rel_item")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub container_id: Uuid,
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::container::Entity",
            from = "Column::ContainerId",
            to = "super::container::Column::Id"
        )]
        Container,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

struct ItemsService;

impl CrudService for ItemsService {
    type Entity = item::Entity;
}

struct CreateItem {
    container_id: Uuid,
    label: String,
}

impl CreateModel<item::Entity> for CreateItem {
    fn into_active_model(self) -> item::ActiveModel {
        item::ActiveModel {
            id: Set(Uuid::now_v7()),
            container_id: Set(self.container_id),
            label: Set(self.label),
        }
    }
}

impl Creatable for ItemsService {
    type Create = CreateItem;
}

async fn setup_tables(conn: &DatabaseConnection) {
    conn.execute_unprepared(
        "CREATE TABLE rel_container (id uuid PRIMARY KEY, org_id uuid NOT NULL); \
         CREATE TABLE rel_item ( \
             id uuid PRIMARY KEY, \
             container_id uuid NOT NULL REFERENCES rel_container(id), \
             label text NOT NULL \
         );",
    )
    .await
    .expect("create relational test tables");
}

async fn seed_container(conn: &DatabaseConnection, id: Uuid, org_id: Uuid) {
    container::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
    }
    .insert(conn)
    .await
    .expect("seed container");
}

async fn seed_item(conn: &DatabaseConnection, id: Uuid, container_id: Uuid, label: &str) {
    item::ActiveModel {
        id: Set(id),
        container_id: Set(container_id),
        label: Set(label.to_owned()),
    }
    .insert(conn)
    .await
    .expect("seed item");
}

fn items_in_org(org: Uuid) -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org)
        })
    });
    b.can(Action::Create, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org)
        })
    });
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn list_returns_only_items_whose_parent_is_in_the_callers_org() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    setup_tables(conn.as_ref()).await;

    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (item_a, item_b) = (Uuid::now_v7(), Uuid::now_v7());
    seed_container(conn.as_ref(), cont_a, org_a).await;
    seed_container(conn.as_ref(), cont_b, org_b).await;
    seed_item(conn.as_ref(), item_a, cont_a, "in A").await;
    seed_item(conn.as_ref(), item_b, cont_b, "in B").await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        with_ability(items_in_org(org_a), async {
            let rows = ItemsService.list().await.expect("list succeeds");
            let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
            assert!(ids.contains(&item_a), "own-org item is visible: {ids:?}");
            assert!(
                !ids.contains(&item_b),
                "cross-org item must be filtered out by the relational scope: {ids:?}",
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn access_distinguishes_found_denied_and_missing() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    setup_tables(conn.as_ref()).await;

    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (item_a, item_b) = (Uuid::now_v7(), Uuid::now_v7());
    seed_container(conn.as_ref(), cont_a, org_a).await;
    seed_container(conn.as_ref(), cont_b, org_b).await;
    seed_item(conn.as_ref(), item_a, cont_a, "in A").await;
    seed_item(conn.as_ref(), item_b, cont_b, "in B").await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        with_ability(items_in_org(org_a), async {
            assert!(
                matches!(
                    ItemsService.access(Action::Read, item_a).await.expect("a"),
                    Access::Found(_)
                ),
                "an item in the caller's org resolves to Found",
            );
            assert!(
                matches!(
                    ItemsService.access(Action::Read, item_b).await.expect("b"),
                    Access::Denied
                ),
                "a cross-org item resolves to Denied, not Missing",
            );
            assert!(
                matches!(
                    ItemsService
                        .access(Action::Read, Uuid::now_v7())
                        .await
                        .expect("missing"),
                    Access::Missing
                ),
                "an absent id resolves to Missing",
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn create_under_an_out_of_scope_parent_is_rejected() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    setup_tables(conn.as_ref()).await;

    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    seed_container(conn.as_ref(), cont_a, org_a).await;
    seed_container(conn.as_ref(), cont_b, org_b).await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        with_ability(items_in_org(org_a), async {
            let ok = ItemsService
                .create(CreateItem {
                    container_id: cont_a,
                    label: "mine".into(),
                })
                .await;
            assert!(ok.is_ok(), "in-scope create succeeds: {ok:?}");

            let denied = ItemsService
                .create(CreateItem {
                    container_id: cont_b,
                    label: "not mine".into(),
                })
                .await;
            assert!(
                matches!(denied, Err(DbErr::RecordNotInserted)),
                "out-of-scope create must be rejected: {denied:?}",
            );
        })
        .await;
    })
    .await;
}
