//! A gateway's messages and connection hooks over `SeaOrmDatabaseModule`: the
//! WS edge runs each as a unit of work through the bound `dyn UnitOfWork`, with
//! no data context listed, under the ability the upgrade's guards attached.

use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_authz::{AbilityBuilder, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_seaorm::{
    Executor, ExecutorScope, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, current_executor,
    current_executor_scope,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::ws::WsApp;
use nest_rs_ws::{WsModule, async_trait, gateway, messages};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;

/// Attaches an ability to every upgrade, as the authorization guard does.
#[injectable]
#[derive(Default)]
struct Granting;

impl Layer for Granting {}

#[async_trait]
impl Guard for Granting {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let ability = AbilityBuilder::new()
            .build()
            .expect("an ability with no rule builds");
        cx.attach(ability);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for Granting {}

/// Write `sql` on the message's or hook's own executor.
async fn write(sql: &str) {
    current_executor()
        .expect("the unit runs with an ambient executor")
        .execute_unprepared(sql)
        .await
        .expect("the write lands in the unit's transaction");
}

#[gateway(path = "/ws/unit")]
#[use_guards(Granting)]
pub(crate) struct UnitGateway;

#[messages]
impl UnitGateway {
    #[subscribe_message("inspect")]
    #[public]
    async fn inspect(&self) -> serde_json::Value {
        json!({
            "lazy": matches!(current_executor(), Some(Executor::Lazy(_))),
            "request": current_executor_scope() == Some(ExecutorScope::Request),
            "ability": current_ability().is_some(),
        })
    }

    #[subscribe_message("commit")]
    #[public]
    async fn commit(&self) -> Result<String, std::io::Error> {
        write("INSERT INTO ws_unit_commit (id) VALUES (1)").await;
        Ok("written".to_owned())
    }

    #[subscribe_message("fail")]
    #[public]
    async fn fail(&self) -> Result<String, std::io::Error> {
        write("INSERT INTO ws_unit_rollback (id) VALUES (1)").await;
        Err(std::io::Error::other("the handler failed after writing"))
    }

    #[subscribe_message("orphan")]
    #[public]
    async fn orphan(&self) -> Result<String, std::io::Error> {
        write("INSERT INTO ws_unit_deferred_children (id, parent_id) VALUES (1, 4242)").await;
        Ok("written".to_owned())
    }
}

static CONNECTED_WITH_ABILITY: AtomicBool = AtomicBool::new(false);

#[gateway(path = "/ws/unit/connect-writes")]
#[use_guards(Granting)]
pub(crate) struct ConnectWritesGateway;

#[messages]
impl ConnectWritesGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".to_owned()
    }

    #[on_connect]
    async fn joined(&self) {
        CONNECTED_WITH_ABILITY.store(current_ability().is_some(), Ordering::SeqCst);
        write("INSERT INTO ws_unit_connect_commit (id) VALUES (1)").await;
    }
}

#[gateway(path = "/ws/unit/connect-panics")]
#[use_guards(Granting)]
pub(crate) struct ConnectPanicsGateway;

#[messages]
impl ConnectPanicsGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".to_owned()
    }

    #[on_connect]
    async fn joined(&self) {
        write("INSERT INTO ws_unit_connect_rollback (id) VALUES (1)").await;
        panic!("the connect hook failed after writing");
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        WsModule,
    ],
    providers = [Granting, UnitGateway, ConnectWritesGateway, ConnectPanicsGateway],
)]
struct UnitModule;

async fn boot() -> WsApp {
    TestApp::builder()
        .module::<UnitModule>()
        .build_ws()
        .await
        .expect("the gateways boot over live Postgres")
}

async fn table(conn: &DatabaseConnection, name: &str) {
    for sql in [
        format!("DROP TABLE IF EXISTS {name}"),
        format!("CREATE TABLE {name} (id INT PRIMARY KEY)"),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("set up the probe table `{name}`: {err}"));
    }
}

async fn ask(app: &WsApp, event: &str) -> serde_json::Value {
    let mut socket = app.socket("/ws/unit").connect().await;
    socket.send(event, json!(null)).await;
    socket.next_envelope().await
}

#[tokio::test]
async fn a_gateway_listing_no_data_context_runs_each_message_in_a_lazy_transaction() {
    let app = boot().await;
    let answer = ask(&app, "inspect").await;
    assert_eq!(
        answer["data"],
        json!({ "lazy": true, "request": true, "ability": true }),
        "the edge opened the message's unit, under the ability the upgrade attached: {answer}",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_success_reply_commits_the_messages_writes() {
    let conn = crate::harness::connect().await;
    table(&conn, "ws_unit_commit").await;
    let app = boot().await;

    let answer = ask(&app, "commit").await;
    assert_eq!(answer["data"], "written", "{answer}");
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_commit").await,
        1
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn an_error_reply_rolls_back_the_messages_writes() {
    let conn = crate::harness::connect().await;
    table(&conn, "ws_unit_rollback").await;
    let app = boot().await;

    let answer = ask(&app, "fail").await;
    assert_eq!(
        answer["data"]["error"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "the handler's error is the reply: {answer}"
    );
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_rollback").await,
        0,
        "an error reply leaves nothing of what the message wrote",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_message_whose_commit_the_database_refuses_is_answered_with_an_error() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect().await;
    let children = crate::harness::deferred_probe_tables(&conn, "ws_unit_deferred").await;
    conn.execute_unprepared(&format!("DELETE FROM {children}"))
        .await
        .expect("no row is left from an earlier run");
    let app = boot().await;

    let answer = ask(&app, "orphan").await;
    assert_eq!(
        answer["data"]["error"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "a success whose transaction did not commit is answered as an internal error: {answer}",
    );
    assert_eq!(crate::harness::committed_rows(&conn, &children).await, 0);
    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "transaction commit failed");
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("transport").as_deref(),
        Some("ws"),
        "{event:#?}"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_connect_hook_writes_in_a_unit_of_its_own_under_the_upgrade_ability() {
    let conn = crate::harness::connect().await;
    table(&conn, "ws_unit_connect_commit").await;
    let app = boot().await;

    let mut socket = app.socket("/ws/unit/connect-writes").connect().await;
    socket.send("ping", json!(null)).await;
    assert_eq!(socket.next_envelope().await["data"], "pong");

    assert!(
        CONNECTED_WITH_ABILITY.load(Ordering::SeqCst),
        "the hook runs under the ability the upgrade attached",
    );
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_connect_commit").await,
        1,
        "a hook that ran to its end commits what it wrote",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_connect_hook_that_panics_after_writing_leaves_nothing() {
    let conn = crate::harness::connect().await;
    table(&conn, "ws_unit_connect_rollback").await;
    let app = boot().await;

    let mut socket = app.socket("/ws/unit/connect-panics").connect().await;
    let (code, _) = socket.expect_close().await;
    assert_eq!(code, nest_rs_ws::CloseCode::Error, "§7.4.1 1011");
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_connect_rollback").await,
        0,
        "the hook's unit rolled back with its panic",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

/// The floor every per-unit timeout is held to: a second.
const MESSAGE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);

static WROTE_THEN_STALLED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[gateway(path = "/ws/unit/deadline")]
#[use_guards(Granting)]
pub(crate) struct DeadlineGateway;

#[messages]
impl DeadlineGateway {
    #[subscribe_message("stall")]
    #[public]
    async fn stall(&self) -> String {
        write("INSERT INTO ws_unit_deadline (id) VALUES (1)").await;
        WROTE_THEN_STALLED.notify_one();
        std::future::pending::<()>().await;
        "never".to_owned()
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        WsModule::for_root(nest_rs_ws::WsConfig {
            message_timeout: Some(MESSAGE_DEADLINE),
            ..nest_rs_ws::WsConfig::default()
        }),
    ],
    providers = [Granting, DeadlineGateway],
)]
struct DeadlineModule;

/// The deadline cuts the message's body inside its unit, so the unit settles
/// the cut as a failure: a rollback, never a commit of half the work, nor a
/// transaction abandoned unsettled.
#[tokio::test]
async fn a_message_that_wrote_then_outlived_its_deadline_leaves_no_row() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect().await;
    table(&conn, "ws_unit_deadline").await;
    let app = TestApp::builder()
        .module::<DeadlineModule>()
        .build_ws()
        .await
        .expect("the gateway boots over live Postgres");

    let mut socket = app.socket("/ws/unit/deadline").connect().await;
    socket.send("stall", json!(null)).await;
    WROTE_THEN_STALLED.notified().await;
    // Paused only once the write landed: the deadline is the framework's own.
    tokio::time::pause();
    tokio::time::sleep(MESSAGE_DEADLINE + std::time::Duration::from_millis(1)).await;
    tokio::time::resume();

    let answer = socket.next_envelope().await;
    assert_eq!(answer["data"]["errors"]["reason"], "timeout", "{answer}");
    // The close drains the binding's settles: the count follows the cut unit's.
    drop(socket);
    app.close().await;
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_deadline").await,
        0,
        "the cut message's write was rolled back",
    );
    logs.expect_one(
        nest_rs_ws::TARGET,
        "websocket message not answered within its deadline; it is cut",
    );
    let abandoned: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| {
            event.target == nest_rs_seaorm::target::ORM
                && event.field("outcome").as_deref() == Some("abandoned")
        })
        .collect();
    assert!(
        abandoned.is_empty(),
        "the unit settled its transaction as a rollback: {abandoned:#?}",
    );
}

static BLOCKED_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[gateway(path = "/ws/unit/blocked")]
#[use_guards(Granting)]
pub(crate) struct BlockedGateway;

#[messages]
impl BlockedGateway {
    #[subscribe_message("write")]
    #[public]
    async fn write_blocked(&self) -> String {
        BLOCKED_STARTED.notify_one();
        write("INSERT INTO ws_unit_blocked (id) VALUES (1)").await;
        "written".to_owned()
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        WsModule::for_root(nest_rs_ws::WsConfig {
            message_timeout: Some(MESSAGE_DEADLINE),
            ..nest_rs_ws::WsConfig::default()
        }),
    ],
    providers = [Granting, BlockedGateway],
)]
struct BlockedModule;

/// A message whose statement Postgres holds behind a lock is answered at its
/// deadline, not when the lock goes: the rollback waits for the statement on
/// the driver's task, and nothing it wrote lands.
#[tokio::test]
async fn a_message_whose_statement_is_blocked_is_answered_at_its_deadline() {
    use sea_orm::TransactionTrait;

    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect().await;
    table(&conn, "ws_unit_blocked").await;
    let locker = conn.begin().await.expect("the locking transaction opens");
    locker
        .execute_unprepared("LOCK TABLE ws_unit_blocked IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("the table is locked");
    let app = TestApp::builder()
        .module::<BlockedModule>()
        .build_ws()
        .await
        .expect("the gateway boots over live Postgres");

    let mut socket = app.socket("/ws/unit/blocked").connect().await;
    socket.send("write", json!(null)).await;
    BLOCKED_STARTED.notified().await;
    let sent = std::time::Instant::now();
    let answer = socket.next_envelope().await;
    let took = sent.elapsed();
    assert_eq!(answer["data"]["errors"]["reason"], "timeout", "{answer}");
    assert!(
        took < MESSAGE_DEADLINE + std::time::Duration::from_secs(2),
        "answered at its deadline while the lock is still held, took {took:?}",
    );

    locker.rollback().await.expect("the lock goes");
    // The close drains the binding's settles: the count follows the cut unit's.
    drop(socket);
    app.close().await;
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_blocked").await,
        0,
        "the cut message's write was rolled back once the statement ran",
    );
    let abandoned: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| {
            event.target == nest_rs_seaorm::target::ORM
                && event.field("outcome").as_deref() == Some("abandoned")
        })
        .collect();
    assert!(
        abandoned.is_empty(),
        "settled, never abandoned: {abandoned:#?}"
    );
}
