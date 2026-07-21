//! `Creatable::create` is atomic with its scope re-check on **every**
//! executor shape. The WS message path (and any bare `with_executor` on the
//! pool) has no ambient request transaction, so `create` opens a local one —
//! an out-of-scope insert must surface `RecordNotInserted` and leave zero
//! rows behind.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action, with_ability};
use nest_rs_seaorm::{Creatable, CreateModel, CrudService, Executor, with_request_executor};
use sea_orm::prelude::Uuid;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set};

mod gadget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "create_scope_gadgets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct CreateGadget {
    id: Uuid,
    org_id: i32,
}

impl CreateModel<gadget::Entity> for CreateGadget {
    fn into_active_model(self) -> gadget::ActiveModel {
        gadget::ActiveModel {
            id: Set(self.id),
            org_id: Set(self.org_id),
        }
    }
}

struct GadgetsService;

impl CrudService for GadgetsService {
    type Entity = gadget::Entity;
}

impl Creatable for GadgetsService {
    type Create = CreateGadget;
}

async fn db() -> DatabaseConnection {
    // Both tests race this setup, and nextest runs each in its own process, so
    // the serialization has to happen in Postgres — see `setup_shared_table`.
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "create_scope_gadgets",
        "CREATE TABLE IF NOT EXISTS create_scope_gadgets (
            id UUID PRIMARY KEY,
            org_id INT NOT NULL
        );",
    )
    .await;
    conn
}

fn org_scoped_ability(org: i32) -> Arc<nest_rs_authz::Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Create, gadget::Entity)
        .when(move |p| p.eq(gadget::Column::OrgId, org));
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn out_of_scope_create_over_the_pool_executor_persists_nothing() {
    let conn = db().await;
    let id = Uuid::now_v7();

    // The WS-message executor shape: the shared pool, request-tagged, with an
    // ambient ability — and no surrounding transaction to roll anything back.
    let result = with_request_executor(
        Executor::Pool(conn.clone()),
        with_ability(org_scoped_ability(1), async {
            GadgetsService.create(CreateGadget { id, org_id: 2 }).await
        }),
    )
    .await;

    assert!(
        matches!(result, Err(DbErr::RecordNotInserted)),
        "an out-of-scope create must surface RecordNotInserted, got {result:?}",
    );
    let persisted = gadget::Entity::find()
        .filter(gadget::Column::Id.eq(id))
        .one(&conn)
        .await
        .expect("count query runs");
    assert!(
        persisted.is_none(),
        "the out-of-scope row must not persist on a pool executor",
    );
}

#[tokio::test]
async fn in_scope_create_over_the_pool_executor_commits() {
    let conn = db().await;
    let id = Uuid::now_v7();

    let result = with_request_executor(
        Executor::Pool(conn.clone()),
        with_ability(org_scoped_ability(7), async {
            GadgetsService.create(CreateGadget { id, org_id: 7 }).await
        }),
    )
    .await;

    let model = result.expect("an in-scope create succeeds");
    assert_eq!(model.org_id, 7);
    let persisted = gadget::Entity::find()
        .filter(gadget::Column::Id.eq(id))
        .one(&conn)
        .await
        .expect("read-back query runs")
        .expect("the committed row is visible outside the local transaction");
    assert_eq!(persisted.id, id);

    // Cleanup so reruns stay idempotent.
    gadget::Entity::delete_by_id(id)
        .exec(&conn)
        .await
        .expect("cleanup");
}
