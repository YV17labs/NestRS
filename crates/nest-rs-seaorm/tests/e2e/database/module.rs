//! Covers `src/database/module.rs`: the binding declares the pool's budget
//! ambient, so every net reaches it, a strategy injecting nothing included; and
//! it runs every lifecycle hook as a unit of work of its own, so a hook reaches
//! `Repo` through its service, its writes stand on its `Ok` alone, and a
//! listener it emits to runs in a unit of its own.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
use nest_rs_authz::Action;
use nest_rs_core::{App, BudgetPastNetError, Module, hooks, injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_seaorm::{
    Creatable, CreateModel, CrudService, Repo, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule,
};
use poem::Request;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, Set,
};

/// A strategy that injects nothing, so only the pool's ambience reaches it.
#[injectable]
struct LocalStrategy;

#[async_trait]
impl Strategy for LocalStrategy {
    type Principal = crate::Anyone;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

type LocalGuard = AuthnGuard<LocalStrategy>;

#[module(
    imports = [SeaOrmModule::for_root(None), SeaOrmDatabaseModule],
    providers = [LocalStrategy, LocalGuard],
)]
struct BoundPoolModule;

#[tokio::test]
async fn a_pool_budget_at_the_authentication_guards_net_fails_the_boot() {
    let Err(refused) = App::builder()
        .provide(crate::module::config(Some(AUTHENTICATE_TIMEOUT)))
        .module::<BoundPoolModule>()
        .build()
        .await
    else {
        panic!("a pool `Repo` reaches from the strategy must not wait as long as the guard");
    };
    let refused = refused
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.port),
        ("the SeaORM pool", "the authentication guard")
    );
}

mod row {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "hook_unit_rows")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct CreateRow(&'static str);

impl CreateModel<row::Entity> for CreateRow {
    fn into_active_model(self) -> row::ActiveModel {
        row::ActiveModel {
            id: Set(Uuid::now_v7()),
            label: Set(self.0.to_owned()),
        }
    }
}

#[injectable]
#[derive(Default)]
struct RowsService;

impl CrudService for RowsService {
    type Entity = row::Entity;
}

impl Creatable for RowsService {
    type Create = CreateRow;
}

/// The suite's pool, the rows table ready and every row labelled `label`
/// gone, so a count is this test's alone.
async fn pool_without(label: &str) -> Arc<DatabaseConnection> {
    let conn = crate::harness::connect_arc().await;
    crate::harness::setup_shared_table(
        &conn,
        "hook_unit_rows",
        "CREATE TABLE IF NOT EXISTS hook_unit_rows (
            id UUID PRIMARY KEY,
            label TEXT NOT NULL
        );",
    )
    .await;
    row::Entity::delete_many()
        .filter(row::Column::Label.eq(label))
        .exec(&*conn)
        .await
        .expect("the test's rows are cleared");
    conn
}

/// The rows labelled `label` that committed, counted outside every unit.
async fn committed(conn: &DatabaseConnection, label: &str) -> u64 {
    row::Entity::find()
        .filter(row::Column::Label.eq(label))
        .count(conn)
        .await
        .expect("the rows are counted")
}

/// An app over the suite's pool, bound by the SeaORM binding.
async fn booted<M: Module>(conn: &Arc<DatabaseConnection>) -> App {
    App::builder()
        .provide_arc(Arc::clone(conn))
        .provide(SeaOrmConfig {
            url: crate::harness::url(),
            ..SeaOrmConfig::default()
        })
        .module::<M>()
        .build()
        .await
        .expect("the module boots")
}

const SEEDED: &str = "hook-seeded";

#[injectable]
struct SeedsThroughItsService {
    #[inject]
    svc: Arc<RowsService>,
}

#[hooks]
impl SeedsThroughItsService {
    #[on_module_init]
    async fn seed(&self) -> anyhow::Result<()> {
        self.svc.create(CreateRow(SEEDED)).await?;
        Ok(())
    }
}

#[module(imports = [SeaOrmDatabaseModule], providers = [RowsService, SeedsThroughItsService])]
struct SeedingModule;

#[tokio::test]
async fn an_init_hook_writes_through_its_services_repo_and_its_ok_commits() {
    let conn = pool_without(SEEDED).await;
    let app = booted::<SeedingModule>(&conn).await;
    app.init()
        .await
        .expect("the hook's unit gives `Repo` its executor");
    app.close().await;
    assert_eq!(committed(&conn, SEEDED).await, 1);
}

const REFUSED: &str = "hook-refused";

#[injectable]
struct WritesThenRefuses {
    #[inject]
    svc: Arc<RowsService>,
}

#[hooks]
impl WritesThenRefuses {
    #[on_module_init]
    async fn seed(&self) -> anyhow::Result<()> {
        self.svc.create(CreateRow(REFUSED)).await?;
        anyhow::bail!("the seed's second half found the data inconsistent")
    }
}

#[module(imports = [SeaOrmDatabaseModule], providers = [RowsService, WritesThenRefuses])]
struct RefusingModule;

#[tokio::test]
async fn an_init_hook_that_writes_then_answers_err_leaves_no_row() {
    let conn = pool_without(REFUSED).await;
    let app = booted::<RefusingModule>(&conn).await;
    let refused = app.init().await.expect_err("the hook fails the boot");
    assert!(
        format!("{refused:#}").contains("found the data inconsistent"),
        "{refused:#}"
    );
    app.close().await;
    assert_eq!(
        committed(&conn, REFUSED).await,
        0,
        "the hook's unit rolled back what it wrote"
    );
}

const COUNTED: &str = "hook-counted";

/// What the shutdown hook read, seeded so the test still holds it once the
/// app is gone.
#[derive(Default)]
struct ShutdownReading(Mutex<Option<u64>>);

#[injectable]
struct CountsAtShutdown {
    #[inject]
    reading: Arc<ShutdownReading>,
}

#[hooks]
impl CountsAtShutdown {
    #[on_application_shutdown]
    async fn report(&self) -> anyhow::Result<()> {
        let counted = Repo::<row::Entity>::scoped(Action::Read)
            .filter(row::Column::Label.eq(COUNTED))
            .count(&Repo::<row::Entity>::conn()?)
            .await?;
        *self
            .reading
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(counted);
        Ok(())
    }
}

#[module(imports = [SeaOrmDatabaseModule], providers = [CountsAtShutdown])]
struct CountingModule;

#[tokio::test]
async fn a_shutdown_hook_reads_through_repo() {
    let conn = pool_without(COUNTED).await;
    conn.execute_unprepared(&format!(
        "INSERT INTO hook_unit_rows (id, label) VALUES ('{}', '{COUNTED}')",
        Uuid::now_v7()
    ))
    .await
    .expect("the row the hook counts is seeded");
    let reading = Arc::new(ShutdownReading::default());
    let app = App::builder()
        .provide_arc(Arc::clone(&conn))
        .provide(SeaOrmConfig {
            url: crate::harness::url(),
            ..SeaOrmConfig::default()
        })
        .provide_arc(Arc::clone(&reading))
        .module::<CountingModule>()
        .build()
        .await
        .expect("the module boots");
    app.init().await.expect("the init phases drain");
    app.close().await;
    assert_eq!(
        *reading.0.lock().unwrap_or_else(PoisonError::into_inner),
        Some(1),
        "the shutdown hook read through `Repo` in its unit",
    );
    row::Entity::delete_many()
        .filter(row::Column::Label.eq(COUNTED))
        .exec(&*conn)
        .await
        .expect("cleanup");
}

const EMITTER: &str = "hook-emitter";
const LISTENER_OK: &str = "hook-listener-ok";
const LISTENER_ERR: &str = "hook-listener-err";

#[derive(Clone)]
struct RowsAsked {
    label: &'static str,
    fails: bool,
}

#[injectable]
struct EmitsOnInit {
    #[inject]
    svc: Arc<RowsService>,
    #[inject]
    bus: Arc<EventBus>,
}

#[hooks]
impl EmitsOnInit {
    #[on_module_init]
    async fn announce(&self) -> anyhow::Result<()> {
        self.svc.create(CreateRow(EMITTER)).await?;
        for (label, fails) in [(LISTENER_ERR, true), (LISTENER_OK, false)] {
            self.bus.emit(RowsAsked { label, fails }).await;
        }
        Ok(())
    }
}

#[injectable]
struct RowsListeners {
    #[inject]
    svc: Arc<RowsService>,
}

#[listeners]
impl RowsListeners {
    #[on_event]
    async fn writes_two(&self, asked: RowsAsked) -> anyhow::Result<()> {
        self.svc.create(CreateRow(asked.label)).await?;
        self.svc.create(CreateRow(asked.label)).await?;
        anyhow::ensure!(!asked.fails, "the reaction refused after writing");
        Ok(())
    }
}

#[module(
    imports = [SeaOrmDatabaseModule, EventsModule],
    providers = [RowsService, EmitsOnInit, RowsListeners],
)]
struct EmittingModule;

#[tokio::test]
async fn a_listener_a_hook_emits_to_runs_in_a_unit_of_its_own_settled_on_its_answer() {
    let conn = pool_without(EMITTER).await;
    for label in [LISTENER_OK, LISTENER_ERR] {
        row::Entity::delete_many()
            .filter(row::Column::Label.eq(label))
            .exec(&*conn)
            .await
            .expect("the test's rows are cleared");
    }
    let app = booted::<EmittingModule>(&conn).await;
    app.init()
        .await
        .expect("the hook committed; a listener's failure is its own");
    app.close().await;
    assert_eq!(committed(&conn, EMITTER).await, 1, "the hook's own write");
    assert_eq!(
        committed(&conn, LISTENER_ERR).await,
        0,
        "the listener answering Err rolled both its writes back",
    );
    assert_eq!(
        committed(&conn, LISTENER_OK).await,
        2,
        "the listener answering Ok committed both writes, in a transaction of its own",
    );
}
