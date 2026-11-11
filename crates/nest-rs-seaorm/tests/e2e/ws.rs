//! A gateway's messages and connection hooks over `SeaOrmDatabaseModule`: the
//! WS edge runs each as a unit of work through the bound `dyn UnitOfWork`, with
//! no data context listed, under the ability the upgrade's guards attached.

use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_authz::{AbilityBuilder, Action, Read, current_ability, subject};
use nest_rs_core::{Layer, async_trait, injectable, module};
use nest_rs_database::{ExecutorScope, current_executor_scope};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_seaorm::__private::{SeaOrmExecutor, current_executor};
use nest_rs_seaorm::{Repo, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::TestApp;
use nest_rs_testing::ws::WsApp;
use nest_rs_ws::{WsModule, gateway, messages};
use sea_orm::{ActiveValue::Set, ConnectionTrait};
use serde_json::json;

/// What `inspect` is gated on: a capability, whose posture opens the unit's
/// reads to the ability its guards kept, as an entity's does.
#[subject]
struct Inspection;

/// Attaches an ability to every upgrade, as the authorization guard does.
#[injectable]
#[derive(Default, Layer)]
struct Granting;

#[async_trait]
impl Guard for Granting {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let mut ability = AbilityBuilder::new();
        ability.can(Action::Read, Inspection);
        let ability = ability.build().expect("one grant builds");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
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
    #[public(Read, Inspection)]
    async fn inspect(&self) -> serde_json::Value {
        json!({
            "lazy": matches!(current_executor(), Some(SeaOrmExecutor::Lazy(_))),
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

/// A row a connection hook reaches for through `Repo`.
mod hook_row {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "ws_unit_hook_rows")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// The ability the upgrade attached grants the row whole, so what a hook meets is the
/// hook's own bar and no missing grant.
#[injectable]
#[derive(Default, Layer)]
struct GrantingRows;

#[async_trait]
impl Guard for GrantingRows {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let mut ability = AbilityBuilder::new();
        ability.can(Action::Read, hook_row::Entity);
        ability.can(Action::Update, hook_row::Entity);
        let ability = ability.build().expect("two grants build");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for GrantingRows {}

/// What the hook's read found and how its by-id write ended.
static HOOK_REPO: std::sync::Mutex<Option<(usize, bool)>> = std::sync::Mutex::new(None);

#[gateway(path = "/ws/unit/connect-repo")]
#[use_guards(GrantingRows)]
pub(crate) struct ConnectRepoGateway;

#[messages]
impl ConnectRepoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".to_owned()
    }

    #[on_connect]
    async fn joined(&self) {
        let found = Repo::<hook_row::Entity>::all()
            .await
            .expect("the read runs");
        let written = Repo::<hook_row::Entity>::update(hook_row::ActiveModel {
            id: Set(1),
            label: Set("rewritten".to_owned()),
        })
        .await;
        *HOOK_REPO.lock().expect("no test poisons it") = Some((
            found.len(),
            matches!(written, Err(sea_orm::DbErr::RecordNotUpdated)),
        ));
    }
}

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
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        WsModule,
    ],
    providers = [
        Granting,
        GrantingRows,
        UnitGateway,
        ConnectWritesGateway,
        ConnectPanicsGateway,
        ConnectRepoGateway,
    ],
)]
struct UnitModule;

async fn boot() -> WsApp {
    TestApp::builder()
        .module::<UnitModule>()
        .build_ws()
        .await
        .expect("the gateways boot over live Postgres")
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
    crate::harness::fresh_table(&conn, "ws_unit_commit").await;
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
    crate::harness::fresh_table(&conn, "ws_unit_rollback").await;
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
async fn a_connect_hook_writes_in_a_unit_of_its_own_and_reads_under_no_ability() {
    let conn = crate::harness::connect().await;
    crate::harness::fresh_table(&conn, "ws_unit_connect_commit").await;
    let app = boot().await;

    let mut socket = app.socket("/ws/unit/connect-writes").connect().await;
    socket.send("ping", json!(null)).await;
    assert_eq!(socket.next_envelope().await["data"], "pong");

    assert!(
        !CONNECTED_WITH_ABILITY.load(Ordering::SeqCst),
        "a hook states no posture naming its reads, so the ability the upgrade attached \
         stays off it",
    );
    assert_eq!(
        crate::harness::committed_rows(&conn, "ws_unit_connect_commit").await,
        1,
        "a hook that ran to its end commits what it wrote",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

/// A hook's read answers nothing and its by-id write touches nothing, whatever the
/// ability the upgrade attached grants: a hook states no posture naming its reads.
#[tokio::test]
async fn a_connect_hooks_repo_reads_no_row_and_writes_none() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "ws_unit_hook_rows",
        "CREATE TABLE IF NOT EXISTS ws_unit_hook_rows (id INT PRIMARY KEY, label TEXT NOT NULL);
         INSERT INTO ws_unit_hook_rows (id, label) VALUES (1, 'seeded')
         ON CONFLICT (id) DO UPDATE SET label = 'seeded';",
    )
    .await;
    let app = boot().await;

    let mut socket = app.socket("/ws/unit/connect-repo").connect().await;
    socket.send("ping", json!(null)).await;
    assert_eq!(socket.next_envelope().await["data"], "pong");

    assert_eq!(
        *HOOK_REPO.lock().expect("no test poisons it"),
        Some((0, true)),
        "no row read, and the by-id write met no row",
    );
    let label = conn
        .query_one_raw(sea_orm::Statement::from_string(
            conn.get_database_backend(),
            "SELECT label FROM ws_unit_hook_rows WHERE id = 1",
        ))
        .await
        .expect("the read runs")
        .expect("the seeded row stands")
        .try_get::<String>("", "label")
        .expect("a label");
    assert_eq!(label, "seeded", "the write touched nothing");
    assert!(
        !logs
            .find(
                nest_rs_seaorm::target::ORM,
                "no ambient Ability outside a worker job — denying all rows",
            )
            .is_empty(),
        "the read says why it answered nothing",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_connect_hook_that_panics_after_writing_leaves_nothing() {
    let conn = crate::harness::connect().await;
    crate::harness::fresh_table(&conn, "ws_unit_connect_rollback").await;
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
            url: crate::harness::url().into(),
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
    crate::harness::fresh_table(&conn, "ws_unit_deadline").await;
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
    assert_eq!(answer["data"]["reason"], "timeout", "{answer}");
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
            url: crate::harness::url().into(),
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
    crate::harness::fresh_table(&conn, "ws_unit_blocked").await;
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

    // The lock goes only once the message is answered, so the deadline alone
    // can answer it; the read's bound turns a message waiting on the lock into
    // a failure rather than a hang.
    let mut socket = app.socket("/ws/unit/blocked").connect().await;
    socket.send("write", json!(null)).await;
    BLOCKED_STARTED.notified().await;
    let answer = socket.next_envelope().await;
    assert_eq!(
        answer["data"]["reason"], "timeout",
        "answered at its deadline while the lock is still held: {answer}"
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
