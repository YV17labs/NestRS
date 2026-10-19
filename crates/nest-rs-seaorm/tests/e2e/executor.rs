//! What a unit's continuation runs on: a streaming body a mutating route
//! returns reads on the pool once the route's transaction has committed, under
//! the caller's ability; an `#[sse]` stream, which no mask shapes, reads
//! nothing scoped; a listener with no commit to wait for runs at once, in a
//! unit of its own beside the transaction the caller opened itself.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::http::Authorize;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read, WireModelDefaults};
use nest_rs_core::module;
use nest_rs_guards::guard;
use nest_rs_http::futures_util::stream;
use nest_rs_http::{Body, HeaderValue, Response, SseEvent, SseStream};
use nest_rs_http::{controller, routes};
use nest_rs_seaorm::{CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::TestApp;
use sea_orm::ConnectionTrait;

const TABLE: &str = "continuation_probe_items";
const STREAMED: &str = "sse_probe_items";

mod item {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "continuation_probe_items")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub published: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for item::Entity {}

struct ItemsService;

impl CrudService for ItemsService {
    type Entity = item::Entity;
}

mod streamed {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "sse_probe_items")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub published: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for streamed::Entity {}

struct StreamedService;

impl CrudService for StreamedService {
    type Entity = streamed::Entity;
}

/// A visitor reads published items, and nothing else.
#[nest_rs_core::injectable]
#[derive(Default)]
struct PublishedItems;

impl AbilityFactory for PublishedItems {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, _ab: &mut AbilityBuilder) {}

    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, item::Entity)
            .when(|p| p.eq(item::Column::Published, true));
        ab.can(Action::Read, streamed::Entity)
            .when(|p| p.eq(streamed::Column::Published, true));
    }
}

type ItemsGuard = AbilityGuard<PublishedItems>;

#[controller(path = "/items")]
#[use_guards(ItemsGuard)]
struct ItemsController;

#[routes]
impl ItemsController {
    /// Writes a published item in the route's transaction, then streams what
    /// the caller may read once the body is polled — after the commit.
    #[post("/stream")]
    #[public]
    async fn stream(&self, _authz: Authorize<Read, item::Entity>) -> Response {
        nest_rs_seaorm::current_executor()
            .expect("a mutating route runs in a transaction")
            .execute_unprepared(&format!(
                "INSERT INTO {TABLE} (id, published) VALUES (3, true)"
            ))
            .await
            .expect("the write lands in the route's transaction");
        let listed = stream::once(async {
            let line = match ItemsService.list().await {
                Ok(rows) => {
                    let mut ids: Vec<i32> = rows.iter().map(|row| row.id).collect();
                    ids.sort_unstable();
                    let ids: Vec<String> = ids.iter().map(i32::to_string).collect();
                    ids.join(",")
                }
                Err(_) => String::from("failed"),
            };
            Ok::<_, std::io::Error>(nest_rs_http::Bytes::from(line))
        });
        Response::builder()
            .content_type(HeaderValue::from_static("text/plain"))
            .body(Body::from_stream(listed))
    }

    /// Streams how many rows the caller may read, counted once the stream is
    /// polled: an `#[sse]` route takes no mask, so its stream reads under
    /// none.
    #[sse("/events")]
    #[public]
    async fn events(&self) -> SseStream {
        SseStream::new(stream::once(async {
            let line = match StreamedService.list().await {
                Ok(rows) => format!("rows:{}", rows.len()),
                Err(_) => String::from("failed"),
            };
            SseEvent::message(line)
        }))
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
    ],
    providers = [PublishedItems, ItemsGuard, ItemsController],
)]
struct AppModule;

#[tokio::test]
async fn a_streaming_body_reads_on_the_pool_after_the_commit_under_the_callers_ability() {
    let conn = crate::harness::connect().await;
    for sql in [
        format!("DROP TABLE IF EXISTS {TABLE}"),
        format!("CREATE TABLE {TABLE} (id INT PRIMARY KEY, published BOOLEAN NOT NULL)"),
        format!("INSERT INTO {TABLE} (id, published) VALUES (1, true), (2, false)"),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("set up `{TABLE}`: {err}"));
    }
    let app = TestApp::builder()
        .module::<AppModule>()
        .use_guards_global([guard::<ItemsGuard>()])
        .build()
        .await
        .expect("the app boots against live Postgres");

    let resp = app.http().post("/items/stream").send().await;
    resp.assert_status_is_ok();
    let listed = resp.0.into_body().into_string().await.expect("the body");

    assert_eq!(
        listed, "1,3",
        "the body sees the committed write and only the rows the caller may read",
    );
}

/// The visitor may read the published row, through a route a mask shapes; a
/// stream no mask shapes is handed no ability, so the read finds nothing
/// rather than rows nothing would filter.
#[tokio::test]
async fn an_sse_streams_repo_read_fails_closed() {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        STREAMED,
        &format!(
            "CREATE TABLE IF NOT EXISTS {STREAMED} (id INT PRIMARY KEY, published BOOLEAN NOT NULL);
             INSERT INTO {STREAMED} (id, published) VALUES (1, true), (2, false)
                 ON CONFLICT DO NOTHING;"
        ),
    )
    .await;
    let app = TestApp::builder()
        .module::<AppModule>()
        .use_guards_global([guard::<ItemsGuard>()])
        .build()
        .await
        .expect("the app boots against live Postgres");

    let resp = app.http().get("/items/events").send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("the body");

    assert!(
        body.contains("data: rows:0"),
        "the stream reads no row, the published one included: {body:?}",
    );
}

/// What a listener ran on: its emitter's own transaction, or a unit of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RanOn {
    TheCallersTransaction,
    ItsOwnTransaction,
    Another,
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct TxnProbe {
    seen: std::sync::Mutex<Vec<RanOn>>,
}

#[nest_rs_core::injectable]
struct TxnListeners {
    #[inject]
    probe: std::sync::Arc<TxnProbe>,
}

#[derive(Clone)]
struct Recorded;

#[nest_rs_events::listeners]
impl TxnListeners {
    #[on_event]
    async fn on_recorded(&self, _event: Recorded) {
        let ran_on = match nest_rs_seaorm::current_executor() {
            Some(nest_rs_seaorm::Executor::Txn(_)) => RanOn::TheCallersTransaction,
            Some(nest_rs_seaorm::Executor::Lazy(_)) => RanOn::ItsOwnTransaction,
            _ => RanOn::Another,
        };
        self.probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ran_on);
    }
}

#[module(
    imports = [nest_rs_events::EventsModule],
    providers = [TxnProbe, TxnListeners],
)]
struct TxnEventsModule;

/// A request's unit, a safe one: it runs on the pool.
const REQUEST: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
    "nest-rs-http",
    "nest_rs::http",
    nest_rs_core::operation_log::Kind::Server,
    "http.request",
);

/// The caller's commit is its own, so the bus does not wait for it; a listener
/// reached before it is a unit of its own, whose answer settles its writes
/// alone — never a passenger of a transaction it cannot roll back.
#[tokio::test]
async fn a_listener_run_at_once_under_a_callers_own_transaction_runs_in_a_unit_of_its_own() {
    use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
    use sea_orm::TransactionTrait;

    let app = nest_rs_core::App::new::<TxnEventsModule>().expect("boots");
    let bus = app
        .container()
        .get::<nest_rs_events::EventBus>()
        .expect("the bus is provided");
    let probe = app
        .container()
        .get::<TxnProbe>()
        .expect("the probe is provided");
    let conn = crate::harness::connect_arc().await;
    let binding: std::sync::Arc<dyn UnitOfWork> =
        std::sync::Arc::new(nest_rs_seaorm::SeaOrmUnitOfWork::new(
            std::sync::Arc::clone(&conn),
            &SeaOrmConfig::default(),
        ));

    let spec = UnitSpec::new(REQUEST, ExecutorScope::Request, TransactionPolicy::Pool);
    run_unit(
        Some(&binding),
        spec,
        async {
            let txn = conn
                .begin()
                .await
                .expect("the caller opens its own transaction");
            nest_rs_seaorm::with_executor(
                nest_rs_seaorm::Executor::Txn(std::sync::Arc::new(txn)),
                bus.emit(Recorded),
            )
            .await;
            Ok::<(), ()>(())
        },
        Result::is_ok,
        |_| Err(()),
    )
    .await
    .expect("the request's unit settles");

    assert_eq!(
        *probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [RanOn::ItsOwnTransaction],
        "run at once, before the caller's commit, in a transaction of its own",
    );
}

#[module(imports = [SeaOrmModule::for_root(None)])]
struct BoundedPoolModule;

/// A body cut at its deadline leaves its statement running; Postgres cancels
/// it at its bound, and that answer is what the rollback meets. The rollback
/// is said at `warn`, as every statement Postgres cancelled is, whichever edge
/// the boundary served.
#[tokio::test]
async fn a_rollback_after_a_statement_postgres_cancelled_is_said_at_warn() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = nest_rs_core::App::builder()
        .provide(SeaOrmConfig {
            url: crate::harness::url(),
            statement_timeout_secs: Some(1),
            ..SeaOrmConfig::default()
        })
        .module::<BoundedPoolModule>()
        .build()
        .await
        .expect("boots");
    let db = app
        .container()
        .get::<sea_orm::DatabaseConnection>()
        .expect("the pool");
    let lazy = std::sync::Arc::new(nest_rs_seaorm::LazyTransaction::new((*db).clone(), "ws"));
    let executor = nest_rs_seaorm::Executor::Lazy(std::sync::Arc::clone(&lazy));
    let cut = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        executor.execute_unprepared("SELECT pg_sleep(3)"),
    )
    .await;
    assert!(cut.is_err(), "the body is cut while its statement runs");
    drop(executor);

    let settled = lazy.finalize(false).await;
    assert!(
        matches!(settled, nest_rs_seaorm::FinalizeOutcome::RolledBack),
        "{settled:?}"
    );
    let line = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "transaction rolled back after a statement Postgres cancelled; nothing it wrote stands",
    );
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("transport").as_deref(), Some("ws"));
    logs.expect_none(nest_rs_seaorm::target::ORM, "transaction rollback failed");
    app.close().await;
}
