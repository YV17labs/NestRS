//! `Creatable::create` is atomic with its scope re-check on every executor
//! shape, the bare pool (the WS message path) included.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action, with_ability};
use nest_rs_seaorm::{Creatable, CreateModel, CrudService, Executor, with_request_executor};
use sea_orm::prelude::Uuid;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set, TransactionTrait,
};

mod gadget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "create_scope_gadgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

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
    let logs = nest_rs_testing::LogCapture::install();

    // The WS-message shape: the pool, with no surrounding transaction.
    let result = with_request_executor(
        Executor::Pool(conn.clone()),
        with_ability(org_scoped_ability(1), async {
            GadgetsService.create(CreateGadget { id, org_id: 2 }).await
        }),
    )
    .await;

    // `RecordNotInserted` reads like a unique clash: this event alone records
    // the write as an authorization failure.
    let denied = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "access denied — row outside the caller's scope",
    );
    assert_eq!(denied.level, "warn");
    assert_eq!(
        denied.field("entity").as_deref(),
        Some("create_scope_gadgets")
    );

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

/// A `SAVEPOINT` that cannot open poisons the boundary, so a swallowed `DbErr`
/// never settles as `NoTransaction`. The create must be the boundary's first
/// data-layer touch, so its acquire is the one that times out.
#[tokio::test]
async fn a_create_that_cannot_open_its_savepoint_poisons_the_boundary() {
    let held = db().await;
    let starved = crate::harness::starved_pool().await;
    let hog = starved.begin().await.expect("the only connection is held");

    let lazy = Arc::new(nest_rs_seaorm::LazyTransaction::new(starved, "test"));
    let result = with_request_executor(
        Executor::Lazy(Arc::clone(&lazy)),
        with_ability(org_scoped_ability(1), async {
            GadgetsService
                .create(CreateGadget {
                    id: Uuid::now_v7(),
                    org_id: 1,
                })
                .await
        }),
    )
    .await;
    assert!(
        result.is_err(),
        "the create cannot open its SAVEPOINT on an exhausted pool, got {result:?}",
    );

    // Swallowed, as `let _ = svc.create(..)` would.
    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(
            outcome,
            nest_rs_seaorm::FinalizeOutcome::Poisoned { retryable: true }
        ),
        "a boundary that reported success over a create which never opened must \
         settle as poisoned, not as `NoTransaction` — and *retryable*, since a \
         pool that handed out no connection ran no statement and left nothing \
         behind, got {outcome:?}",
    );

    hog.rollback().await.expect("the held connection is freed");
    drop(held);
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

    gadget::Entity::delete_by_id(id)
        .exec(&conn)
        .await
        .expect("cleanup");
}

// A trigger terminates its own backend: the insert fails and the session its
// SAVEPOINT would roll back on is gone, as a `pg_terminate_backend` leaves it.
mod fragile {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "create_rollback_gadgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct CreateFragile {
    id: Uuid,
    org_id: i32,
}

impl CreateModel<fragile::Entity> for CreateFragile {
    fn into_active_model(self) -> fragile::ActiveModel {
        fragile::ActiveModel {
            id: Set(self.id),
            org_id: Set(self.org_id),
        }
    }
}

struct FragileService;

impl CrudService for FragileService {
    type Entity = fragile::Entity;
}

impl Creatable for FragileService {
    type Create = CreateFragile;
}

#[tokio::test]
async fn a_create_whose_undo_cannot_be_issued_says_so() {
    let logs = nest_rs_testing::LogCapture::install();
    // Through the shared fixture: nextest's per-test processes race raw
    // `CREATE TABLE` on the Postgres catalog.
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "create_rollback_gadgets",
        "CREATE TABLE IF NOT EXISTS create_rollback_gadgets (
             id UUID PRIMARY KEY, org_id INT NOT NULL
         );
         CREATE OR REPLACE FUNCTION create_rollback_gadgets_kill() RETURNS trigger AS $$
         BEGIN
             PERFORM pg_terminate_backend(pg_backend_pid());
             RETURN NEW;
         END;
         $$ LANGUAGE plpgsql;
         DROP TRIGGER IF EXISTS kill_on_insert ON create_rollback_gadgets;
         CREATE TRIGGER kill_on_insert BEFORE INSERT ON create_rollback_gadgets
             FOR EACH ROW EXECUTE FUNCTION create_rollback_gadgets_kill();",
    )
    .await;

    let mut ability = AbilityBuilder::new();
    ability.can(Action::Create, fragile::Entity);
    let ability = Arc::new(ability.build().expect("a valid ability"));

    let outcome = with_request_executor(
        Executor::Pool(crate::harness::connect().await),
        with_ability(ability, async {
            FragileService
                .create(CreateFragile {
                    id: Uuid::now_v7(),
                    org_id: 1,
                })
                .await
        }),
    )
    .await;

    assert!(
        outcome.is_err(),
        "a create whose insert never landed is an error, whatever happened to the undo",
    );

    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "rollback of the create SAVEPOINT/transaction failed",
    );
    assert_eq!(event.level, "error");
    assert!(
        event.field("entity").is_some_and(|e| e.contains("gadget")),
        "the event names the entity, since the error the caller propagates is \
         the insert's and mentions no boundary at all, got {:?}",
        event.fields,
    );
    assert!(
        event.field("error").is_some(),
        "…and why the undo could not be issued, got {:?}",
        event.fields,
    );
}
