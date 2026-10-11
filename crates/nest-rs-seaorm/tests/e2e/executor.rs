//! What a unit's continuation runs on: a streaming body a mutating route
//! returns reads on the pool once the route's transaction has committed, under
//! the caller's ability; a listener with no commit to wait for runs inside its
//! emitter, in the transaction the caller opened itself.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::http::Authorize;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read, WireModelDefaults};
use nest_rs_core::module;
use nest_rs_guards::guard;
use nest_rs_http::futures_util::stream;
use nest_rs_http::poem::{Body, Response};
use nest_rs_http::{controller, routes};
use nest_rs_seaorm::{CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::TestApp;
use sea_orm::ConnectionTrait;

const TABLE: &str = "continuation_probe_items";

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

/// A visitor reads published items, and nothing else.
#[nest_rs_core::injectable]
#[derive(Default)]
struct PublishedItems;

impl AbilityFactory for PublishedItems {
    type Actor = ();

    fn define(&self, _actor: &(), _ab: &mut AbilityBuilder) {}

    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, item::Entity)
            .when(|p| p.eq(item::Column::Published, true));
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
            Ok::<_, std::io::Error>(line)
        });
        Response::builder()
            .content_type("text/plain")
            .body(Body::from_bytes_stream(listed))
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

/// Whether a listener ran inside its emitter's own transaction.
#[nest_rs_core::injectable]
#[derive(Default)]
struct TxnProbe {
    seen: std::sync::Mutex<Vec<bool>>,
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
        let in_txn = matches!(
            nest_rs_seaorm::current_executor(),
            Some(nest_rs_seaorm::Executor::Txn(_))
        );
        self.probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(in_txn);
    }
}

#[module(
    imports = [nest_rs_events::EventsModule],
    providers = [TxnProbe, TxnListeners],
)]
struct TxnEventsModule;

#[tokio::test]
async fn a_listener_run_at_once_under_a_callers_own_transaction_runs_in_it() {
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
    let txn = crate::harness::connect()
        .await
        .begin()
        .await
        .expect("the caller opens its own transaction");

    let txn = std::sync::Arc::new(txn);
    nest_rs_seaorm::with_executor(
        nest_rs_seaorm::Executor::Txn(std::sync::Arc::clone(&txn)),
        bus.emit(Recorded),
    )
    .await;

    assert_eq!(
        *probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [true],
        "the caller's commit is its own: the listener runs at once, in its transaction",
    );
}
