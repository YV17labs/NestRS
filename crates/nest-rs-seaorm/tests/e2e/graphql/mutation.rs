//! A GraphQL operation over `SeaOrmUnitOfWork`, through `POST /graphql` and
//! the graphql-ws socket: a mutation is one transaction and each of its root
//! fields a savepoint, so a field that errs rolls back its own writes and drops
//! its events while the operation's other fields commit.

use std::sync::{Arc, Mutex, PoisonError};

use nest_rs_core::problem::code;
use nest_rs_core::{injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_graphql::async_graphql::{self, Object};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_seaorm::{
    SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError, current_executor,
};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};

const TABLE: &str = "graphql_mutation_probes";

/// A probe row created: the event a field emits after its write.
#[derive(Clone)]
struct ProbeCreated {
    id: Uuid,
}

/// The ids whose `ProbeCreated` reached its listener.
#[injectable]
#[derive(Default)]
struct Dispatched {
    ids: Mutex<Vec<Uuid>>,
}

impl Dispatched {
    fn holds(&self, id: Uuid) -> bool {
        self.ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
    }
}

#[injectable]
struct DispatchListener {
    #[inject]
    dispatched: Arc<Dispatched>,
}

#[listeners]
impl DispatchListener {
    #[on_event]
    async fn on_created(&self, event: ProbeCreated) {
        self.dispatched
            .ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.id);
    }
}

/// The probe table's one writer.
#[injectable]
#[derive(Default)]
struct ProbeRows;

impl ProbeRows {
    async fn insert(&self, id: Uuid, label: &str) -> Result<(), ServiceError> {
        current_executor()
            .expect("a mutation runs on its unit's executor")
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                format!("INSERT INTO {TABLE} (id, label) VALUES ($1, $2)"),
                [id.into(), label.into()],
            ))
            .await?;
        Ok(())
    }
}

/// What a create answers: a field the schema cannot ship null, which fails.
struct Created;

#[Object]
impl Created {
    async fn checked(&self) -> async_graphql::Result<i32> {
        Err("the sub-field failed".into())
    }
}

#[resolver]
struct ProbesResolver {
    #[inject]
    rows: Arc<ProbeRows>,
    #[inject]
    bus: Arc<EventBus>,
}

#[operations]
impl ProbesResolver {
    #[query]
    #[public]
    async fn up(&self) -> bool {
        true
    }

    /// Insert, emit, then fail when asked: a field that errs after its writes.
    #[mutation]
    #[public]
    async fn create_probe(&self, id: Uuid, fail: bool) -> Result<bool, ServiceError> {
        self.rows.insert(id, &id.to_string()).await?;
        self.bus.emit(ProbeCreated { id }).await;
        match fail {
            true => Err(ServiceError::Invalid("refused after the write".into())),
            false => Ok(true),
        }
    }

    /// Two rows under one label, whose second statement violates its unique key;
    /// nullable, so its sibling's answer stays beside its `null`.
    #[mutation]
    #[public]
    async fn create_twice(
        &self,
        first: Uuid,
        second: Uuid,
        label: String,
    ) -> Result<Option<bool>, ServiceError> {
        self.rows.insert(first, &label).await?;
        self.rows.insert(second, &label).await?;
        Ok(Some(true))
    }

    #[mutation]
    #[public]
    async fn create_checked(&self, id: Uuid) -> Result<Created, ServiceError> {
        self.rows.insert(id, &id.to_string()).await?;
        Ok(Created)
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        EventsModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [ProbeRows, Dispatched, DispatchListener, ProbesResolver],
)]
struct ProbesModule;

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        TABLE,
        &format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (
                id UUID PRIMARY KEY,
                label TEXT UNIQUE NOT NULL
            );"
        ),
    )
    .await;
    conn
}

async fn boot() -> TestApp {
    TestApp::for_module::<ProbesModule>()
        .await
        .expect("the schema boots over the suite's Postgres")
}

/// Answer `body`, a request or a batch, asserting the HTTP status GraphQL
/// answers whatever its fields did.
async fn post(app: &TestApp, body: Value) -> Value {
    let resp = app.http().post("/graphql").body_json(&body).send().await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

async fn stored(conn: &DatabaseConnection, id: Uuid) -> bool {
    conn.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("SELECT id FROM {TABLE} WHERE id = $1"),
        [id.into()],
    ))
    .await
    .expect("the probe reads")
    .is_some()
}

async fn remove(conn: &DatabaseConnection, ids: &[Uuid]) {
    for id in ids {
        conn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("DELETE FROM {TABLE} WHERE id = $1"),
            [(*id).into()],
        ))
        .await
        .expect("the probe row is removed");
    }
}

fn errors_filed(logs: &LogCapture) -> Vec<String> {
    logs.events_at(tracing::Level::ERROR)
        .into_iter()
        .map(|event| format!("{} {}", event.target, event.message))
        .collect()
}

fn create(id: Uuid, fail: bool) -> String {
    format!(r#"createProbe(id: "{id}", fail: {fail})"#)
}

#[tokio::test]
async fn a_field_that_errs_after_its_write_leaves_no_row_and_dispatches_no_event() {
    let conn = db().await;
    let app = boot().await;
    let dispatched = app.container().get::<Dispatched>().expect("provided");
    let (kept, refused) = (Uuid::now_v7(), Uuid::now_v7());

    let reply = post(
        &app,
        json!({ "query": format!("mutation {{ {} }}", create(kept, false)) }),
    )
    .await;
    assert_eq!(reply["data"]["createProbe"], true, "{reply}");
    let reply = post(
        &app,
        json!({ "query": format!("mutation {{ {} }}", create(refused, true)) }),
    )
    .await;
    assert!(reply["errors"].is_array(), "{reply}");

    assert!(
        stored(&conn, kept).await && dispatched.holds(kept),
        "the control commits and dispatches"
    );
    assert!(!stored(&conn, refused).await, "its write rolled back");
    assert!(!dispatched.holds(refused), "its event was dropped with it");
    remove(&conn, &[kept]).await;
}

#[tokio::test]
async fn a_failing_field_rolls_back_alone_and_the_operations_other_fields_commit() {
    let conn = db().await;
    let app = boot().await;
    let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    let reply = post(
        &app,
        json!({
            "query": format!(
                "mutation {{ a: {} b: {} c: {} }}",
                create(a, false),
                create(b, true),
                create(c, false),
            ),
        }),
    )
    .await;

    assert_eq!(reply["errors"][0]["path"], json!(["b"]), "{reply}");
    assert!(stored(&conn, a).await, "{reply}");
    assert!(!stored(&conn, b).await, "{reply}");
    assert!(stored(&conn, c).await, "{reply}");
    remove(&conn, &[a, c]).await;
}

#[tokio::test]
async fn a_unique_violation_answers_its_fields_conflict_and_the_others_commit() {
    let conn = db().await;
    let logs = LogCapture::install();
    let app = boot().await;
    let (kept, first, second) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let label = format!("label-{}", Uuid::now_v7());

    let reply = post(
        &app,
        json!({
            "query": format!(
                r#"mutation {{ kept: {} twice: createTwice(first: "{first}", second: "{second}", label: "{label}") }}"#,
                create(kept, false),
            ),
        }),
    )
    .await;

    assert_eq!(
        reply["data"],
        json!({ "kept": true, "twice": null }),
        "{reply}"
    );
    assert_eq!(reply["errors"][0]["path"], json!(["twice"]), "{reply}");
    assert_eq!(
        reply["errors"][0]["extensions"]["code"],
        code::CONFLICT.as_str(),
        "in band, never an empty 500: {reply}"
    );
    assert!(
        !reply.to_string().contains(&label),
        "the refused value is not quoted: {reply}"
    );
    assert_eq!(
        errors_filed(&logs),
        Vec::<String>::new(),
        "a client's conflict is no incident"
    );
    assert!(stored(&conn, kept).await);
    assert!(
        !stored(&conn, first).await,
        "the field's first statement rolled back with its savepoint"
    );
    remove(&conn, &[kept]).await;
}

#[tokio::test]
async fn a_batch_of_mutations_commits_each_operation_on_its_own_outcome() {
    let conn = db().await;
    let app = boot().await;
    let (kept, refused) = (Uuid::now_v7(), Uuid::now_v7());

    let replies = post(
        &app,
        json!([
            { "query": format!("mutation {{ {} }}", create(kept, false)) },
            { "query": format!("mutation {{ {} }}", create(refused, true)) },
        ]),
    )
    .await;

    assert_eq!(replies[0]["data"]["createProbe"], true, "{replies}");
    assert!(replies[1]["errors"].is_array(), "{replies}");
    assert!(stored(&conn, kept).await);
    assert!(!stored(&conn, refused).await);
    remove(&conn, &[kept]).await;
}

#[tokio::test]
async fn a_mutation_whose_non_null_sub_field_fails_leaves_no_row() {
    let conn = db().await;
    let app = boot().await;
    let id = Uuid::now_v7();

    let reply = post(
        &app,
        json!({ "query": format!(r#"mutation {{ createChecked(id: "{id}") {{ checked }} }}"#) }),
    )
    .await;

    assert!(reply["errors"].is_array(), "{reply}");
    assert!(
        !stored(&conn, id).await,
        "a field whose response carries an error reports no work"
    );
}

/// The next text frame of a graphql-ws socket, parsed.
async fn next_message(socket: &mut nest_rs_testing::ws::WsSocket) -> Value {
    match socket.next_frame().await {
        Some(nest_rs_testing::ws::WsFrame::Text(text)) => {
            serde_json::from_str(&text).expect("a protocol message")
        }
        other => panic!("expected a protocol message, got {other:?}"),
    }
}

#[tokio::test]
async fn over_graphql_ws_a_mutation_that_errs_leaves_no_row_and_one_that_succeeds_commits() {
    let conn = db().await;
    let app = TestApp::builder()
        .module::<ProbesModule>()
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut socket = app
        .socket("/graphql")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .connect()
        .await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    let (kept, refused) = (Uuid::now_v7(), Uuid::now_v7());

    for (id, fail) in [(kept, false), (refused, true)] {
        let query = format!("mutation {{ {} }}", create(id, fail));
        socket
            .send_text(
                json!({ "id": id.to_string(), "type": "subscribe", "payload": { "query": query } })
                    .to_string(),
            )
            .await;
        let answer = next_message(&mut socket).await;
        assert_eq!(answer["type"], "next", "{answer}");
        assert_eq!(next_message(&mut socket).await["type"], "complete");
    }

    assert!(stored(&conn, kept).await, "a socket's mutation commits");
    assert!(!stored(&conn, refused).await, "and rolls back when it errs");
    remove(&conn, &[kept]).await;
}
