//! Relational row-level scoping: an `Ability` rule predicated on a parent
//! entity (`related`) filters reads and by-id writes on the child.

use std::sync::Arc;

use nest_rs_authz::__private::install_ability;
use nest_rs_authz::{Ability, AbilityBuilder, Action};
use nest_rs_database::__private::with_request_executor;
use nest_rs_seaorm::__private::SeaOrmExecutor;
use nest_rs_seaorm::{
    Access, Creatable, CreateModel, CrudService, Deletable, Repo, RepoError, Updatable, UpdateModel,
};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, Set};

mod container {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "rel_container")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod item {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "rel_item")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub container_id: Uuid,
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {
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

struct UpdateItem {
    label: String,
}

impl UpdateModel<item::Entity> for UpdateItem {
    fn apply_to(self, mut model: item::ActiveModel) -> item::ActiveModel {
        model.label = Set(self.label);
        model
    }

    fn reads_back(column: &str) -> bool {
        column == "label"
    }
}

impl Updatable for ItemsService {
    type Update = UpdateItem;
}

impl Deletable for ItemsService {}

async fn setup_tables(conn: &DatabaseConnection) {
    crate::harness::setup_shared_table(
        conn,
        "rel_container",
        "CREATE TABLE IF NOT EXISTS rel_container (id uuid PRIMARY KEY, org_id uuid NOT NULL); \
         CREATE TABLE IF NOT EXISTS rel_item ( \
             id uuid PRIMARY KEY, \
             container_id uuid NOT NULL REFERENCES rel_container(id), \
             label text NOT NULL \
         );",
    )
    .await;
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

struct TwoOrgs {
    org_a: Uuid,
    org_b: Uuid,
    item_a: Uuid,
    item_b: Uuid,
}

/// An item in a container in `org_a`, and one in a container in `org_b`.
async fn seed_two_orgs(conn: &DatabaseConnection) -> TwoOrgs {
    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (item_a, item_b) = (Uuid::now_v7(), Uuid::now_v7());
    seed_container(conn, cont_a, org_a).await;
    seed_container(conn, cont_b, org_b).await;
    seed_item(conn, item_a, cont_a, "in A").await;
    seed_item(conn, item_b, cont_b, "in B").await;
    TwoOrgs {
        org_a,
        org_b,
        item_a,
        item_b,
    }
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
    b.can(Action::Update, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org)
        })
    });
    b.can(Action::Delete, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org)
        })
    });
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn list_returns_only_items_whose_parent_is_in_the_callers_org() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let TwoOrgs {
        org_a,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;

    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(items_in_org(org_a), async {
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
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let TwoOrgs {
        org_a,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;

    let logs = nest_rs_testing::LogCapture::install();
    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(items_in_org(org_a), async {
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
            // `Denied` and `Missing` are the same 404 to the client: the event
            // alone tells them apart.
            let denial = logs.expect_one(nest_rs_seaorm::target::ORM, "access denied");
            assert_eq!(denial.level, "warn");
            assert_eq!(denial.field("entity").as_deref(), Some("rel_item"));
            assert_eq!(denial.field("id"), Some(item_b.to_string()));
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

/// Read on every item, Update only on an item whose container is in `org`: a
/// row read for `Read` cannot show which.
fn reads_all_updates_in_org(org: Uuid) -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, item::Entity);
    b.can(Action::Update, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org)
        })
    });
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn the_instance_check_in_sql_decides_the_relation_a_row_read_for_another_action_hides() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let TwoOrgs {
        org_a,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;

    let ability = reads_all_updates_in_org(org_a);
    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(ability.clone(), async {
            let mine = Repo::<item::Entity>::find_by_id(item_a)
                .await
                .expect("read a")
                .expect("item a is readable");
            let theirs = Repo::<item::Entity>::find_by_id(item_b)
                .await
                .expect("read b")
                .expect("item b is readable");

            assert!(
                !ability.can::<item::Entity>(Action::Update, &mine),
                "the model alone cannot show its container's org",
            );
            assert!(
                Repo::<item::Entity>::can(Action::Update, &mine)
                    .await
                    .expect("check a"),
                "the relation holds: the item's container is in the caller's org",
            );
            assert!(
                !Repo::<item::Entity>::can(Action::Update, &theirs)
                    .await
                    .expect("check b"),
                "the relation does not hold: the item's container is in another org",
            );

            item::Entity::delete_by_id(item_a)
                .exec(&conn)
                .await
                .expect("remove item a");
            assert!(
                !Repo::<item::Entity>::can(Action::Update, &mine)
                    .await
                    .expect("check a again"),
                "a row gone since it was read is not one the caller may update",
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn the_instance_check_in_sql_refuses_the_row_a_relational_denial_names() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let TwoOrgs {
        org_b,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;

    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, item::Entity);
    b.cannot(Action::Update, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org_b)
        })
    });
    let ability = Arc::new(b.build().expect("valid test ability"));

    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(ability.clone(), async {
            let mine = Repo::<item::Entity>::find_by_id(item_a)
                .await
                .expect("read a")
                .expect("item a is readable");
            let theirs = Repo::<item::Entity>::find_by_id(item_b)
                .await
                .expect("read b")
                .expect("item b is readable");

            assert!(
                !ability.can::<item::Entity>(Action::Update, &mine),
                "a denial the model cannot decide denies in memory",
            );
            assert!(
                Repo::<item::Entity>::can(Action::Update, &mine)
                    .await
                    .expect("check a"),
                "SQL decides the denial does not name the item",
            );
            assert!(
                !Repo::<item::Entity>::can(Action::Update, &theirs)
                    .await
                    .expect("check b"),
                "SQL decides the denial names the item",
            );
        })
        .await;
    })
    .await;
}

/// A write says which row it touched, and the one SQL refused says which it
/// refused: `RecordNotUpdated` and a delete of no row read alike to the caller.
#[tokio::test]
async fn a_write_line_names_its_row() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let TwoOrgs {
        org_a,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, item::Entity).when(move |p| {
        p.related::<container::Entity, _>(item::Relation::Container, move |c| {
            c.eq(container::Column::OrgId, org_a)
        })
    });
    let ability = Arc::new(b.build().expect("valid test ability"));
    let mine = item::Entity::find_by_id(item_a)
        .one(&conn)
        .await
        .expect("read a")
        .expect("item a exists");
    let theirs = item::Entity::find_by_id(item_b)
        .one(&conn)
        .await
        .expect("read b")
        .expect("item b exists");

    let logs = nest_rs_testing::LogCapture::install();
    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(ability, async {
            let refused = ItemsService
                .update(
                    theirs.clone(),
                    UpdateItem {
                        label: "not mine".into(),
                    },
                )
                .await;
            assert!(matches!(refused, Err(RepoError::NotFound)), "{refused:?}");
            ItemsService
                .delete(theirs)
                .await
                .expect_err("a row outside the scope is not deleted");
            ItemsService
                .update(
                    mine.clone(),
                    UpdateItem {
                        label: "mine".into(),
                    },
                )
                .await
                .expect("an item in the org updates");
            ItemsService
                .delete(mine)
                .await
                .expect("an item in the org deletes");
        })
        .await;
    })
    .await;

    let refusals = logs.find(
        nest_rs_seaorm::target::ORM,
        "access denied — row outside the caller's scope",
    );
    let refused: Vec<_> = refusals
        .iter()
        .map(|line| (line.field("action"), line.field("id")))
        .collect();
    assert_eq!(
        refused,
        [
            (Some("update".to_owned()), Some(item_b.to_string())),
            (Some("delete".to_owned()), Some(item_b.to_string())),
        ],
    );
    for message in ["row updated", "row deleted"] {
        let served = logs.expect_one(nest_rs_seaorm::target::ORM, message);
        assert_eq!(served.field("id"), Some(item_a.to_string()), "{message}");
    }
}

#[tokio::test]
async fn create_under_an_out_of_scope_parent_is_rejected() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    seed_container(&conn, cont_a, org_a).await;
    seed_container(&conn, cont_b, org_b).await;

    let logs = nest_rs_testing::LogCapture::install();
    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(items_in_org(org_a), async {
            let ok = ItemsService
                .create(CreateItem {
                    container_id: cont_a,
                    label: "mine".into(),
                })
                .await
                .expect("in-scope create succeeds");
            let created = logs.expect_one(nest_rs_seaorm::target::ORM, "row created");
            assert_eq!(created.field("id"), Some(ok.id.to_string()));

            let denied = ItemsService
                .create(CreateItem {
                    container_id: cont_b,
                    label: "not mine".into(),
                })
                .await;
            assert!(
                matches!(denied, Err(RepoError::Forbidden { ref columns, .. }) if columns.is_empty()),
                "out-of-scope create must be rejected: {denied:?}",
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn out_of_scope_update_is_denied_and_leaves_the_row_unchanged() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    let item_b = Uuid::now_v7();
    seed_container(&conn, cont_a, org_a).await;
    seed_container(&conn, cont_b, org_b).await;
    seed_item(&conn, item_b, cont_b, "original").await;

    // Loaded bypassing the ability, so the caller holds a cross-org model.
    let model = item::Entity::find_by_id(item_b)
        .one(&conn)
        .await
        .expect("query")
        .expect("item_b exists");

    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(items_in_org(org_a), async {
            let denied = ItemsService
                .update(
                    model,
                    UpdateItem {
                        label: "hacked".into(),
                    },
                )
                .await;
            assert!(
                matches!(denied, Err(RepoError::NotFound)),
                "an out-of-scope update must be denied: {denied:?}",
            );
        })
        .await;
    })
    .await;

    let after = item::Entity::find_by_id(item_b)
        .one(&conn)
        .await
        .expect("query")
        .expect("item_b still exists");
    assert_eq!(
        after.label, "original",
        "a denied update must not modify the row",
    );
}

#[tokio::test]
async fn out_of_scope_delete_is_denied_and_leaves_the_row() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cont_a, cont_b) = (Uuid::now_v7(), Uuid::now_v7());
    let item_b = Uuid::now_v7();
    seed_container(&conn, cont_a, org_a).await;
    seed_container(&conn, cont_b, org_b).await;
    seed_item(&conn, item_b, cont_b, "keep me").await;

    let model = item::Entity::find_by_id(item_b)
        .one(&conn)
        .await
        .expect("query")
        .expect("item_b exists");

    with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(items_in_org(org_a), async {
            let denied = ItemsService.delete(model).await;
            assert!(
                matches!(denied, Err(RepoError::NotFound)),
                "an out-of-scope delete must be denied: {denied:?}",
            );
        })
        .await;
    })
    .await;

    let survives = item::Entity::find_by_id(item_b)
        .one(&conn)
        .await
        .expect("query");
    assert!(
        survives.is_some(),
        "a denied delete must leave the row in place",
    );
}

/// `list()` truncates at `LIST_CAP` with no signal to the caller, so a `warn`
/// naming the entity is the only trace.
#[tokio::test]
async fn a_list_that_hits_the_hard_cap_says_so() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;

    let org = Uuid::now_v7();
    let container = Uuid::now_v7();
    seed_container(&conn, container, org).await;

    // One past the cap, in a single statement.
    conn.execute_unprepared(&format!(
        "INSERT INTO rel_item (id, container_id, label) \
         SELECT gen_random_uuid(), '{container}', 'row ' || g \
         FROM generate_series(1, {}) AS g",
        nest_rs_seaorm::LIST_CAP + 1,
    ))
    .await
    .expect("seed one row past the cap");

    let logs = nest_rs_testing::LogCapture::install();
    let rows = with_request_executor(SeaOrmExecutor::Pool(conn.clone()).into(), async {
        install_ability(items_in_org(org), async { ItemsService.list().await }).await
    })
    .await
    .expect("the list succeeds — truncation is not an error");

    assert_eq!(
        rows.len() as u64,
        nest_rs_seaorm::LIST_CAP,
        "the caller receives exactly the cap, with nothing marking the cut",
    );

    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "list result truncated at the hard cap",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("entity").as_deref(), Some("rel_item"));
    assert_eq!(
        event.field("cap").as_deref(),
        Some(nest_rs_seaorm::LIST_CAP.to_string()).as_deref(),
    );
}

/// A relational grant narrowed to columns is decided in the write's own statement:
/// a row outside it stands, and a column it withholds sends nothing.
#[tokio::test]
async fn a_relational_grant_limits_what_it_writes_in_the_statement() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;
    let TwoOrgs {
        org_a,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;
    let labels_in_org_a = || {
        let mut b = AbilityBuilder::new();
        b.can(Action::Update, item::Entity)
            .when(move |p| {
                p.related::<container::Entity, _>(item::Relation::Container, move |c| {
                    c.eq(container::Column::OrgId, org_a)
                })
            })
            .fields([item::Column::Label]);
        Arc::new(b.build().expect("valid test ability"))
    };
    let label = |id: Uuid| item::ActiveModel {
        id: Set(id),
        label: Set("relabelled".to_owned()),
        ..Default::default()
    };

    let (own, other, moved) = with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        install_ability(labels_in_org_a(), async {
            (
                Repo::<item::Entity>::update(label(item_a)).await,
                Repo::<item::Entity>::update(label(item_b)).await,
                Repo::<item::Entity>::update(item::ActiveModel {
                    id: Set(item_a),
                    container_id: Set(Uuid::now_v7()),
                    ..Default::default()
                })
                .await,
            )
        }),
    )
    .await;

    assert_eq!(own.expect("own org's label").label, "relabelled");
    assert!(matches!(other, Err(RepoError::NotFound)), "{other:?}");
    assert!(
        matches!(&moved, Err(RepoError::Forbidden { columns, .. }) if columns == &["container_id"]),
        "{moved:?}",
    );
    let untouched = item::Entity::find_by_id(item_b)
        .one(&conn)
        .await
        .expect("read")
        .expect("the row stands");
    assert_eq!(untouched.label, "in B");
}

/// A replacement carrying back all it read sets nothing, so sea-orm sends no `UPDATE`
/// for the grant's scope to sit on: a row outside it answers as with a change.
#[tokio::test]
async fn a_replacement_changing_nothing_on_a_row_outside_its_grant_is_out_of_scope() {
    let conn = crate::harness::connect().await;
    setup_tables(&conn).await;
    let TwoOrgs {
        org_a,
        item_a,
        item_b,
        ..
    } = seed_two_orgs(&conn).await;
    let reading_all_writing_labels_in_org_a = || {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, item::Entity);
        b.can(Action::Update, item::Entity)
            .when(move |p| {
                p.related::<container::Entity, _>(item::Relation::Container, move |c| {
                    c.eq(container::Column::OrgId, org_a)
                })
            })
            .fields([item::Column::Label]);
        Arc::new(b.build().expect("valid test ability"))
    };
    let unchanged = |id: Uuid| {
        let conn = conn.clone();
        async move {
            let model = item::Entity::find_by_id(id)
                .one(&conn)
                .await
                .expect("read")
                .expect("the row stands");
            let input = UpdateItem {
                label: model.label.clone(),
            };
            with_request_executor(
                SeaOrmExecutor::Pool(conn).into(),
                install_ability(
                    reading_all_writing_labels_in_org_a(),
                    ItemsService.update(model, input),
                ),
            )
            .await
        }
    };

    let own = unchanged(item_a).await.expect("its grant admits the row");
    assert_eq!(own.id, item_a);
    let other = unchanged(item_b).await;
    assert!(matches!(other, Err(RepoError::NotFound)), "{other:?}");
}
