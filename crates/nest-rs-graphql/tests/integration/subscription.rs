//! `#[subscription]`: the third discovered root, the graphql-ws mount, and the
//! socket-lifetime ceiling that bounds it.
//!
//! Posture *per item* is tested in `nest-rs-authz`'s
//! `tests/integration/graphql/mask.rs`, where the entity fixtures live.

use std::sync::Arc;
use std::time::Duration;

use async_graphql::SimpleObject;
/// async-graphql's own `futures_util` re-export, reached through the framework.
use async_graphql::futures_util::stream as futures_stream;
use nest_rs_core::module;
use nest_rs_graphql::async_graphql;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_pipes::{Piped, Trim};
use nest_rs_testing::{LogCapture, TestApp};
use tokio::sync::broadcast;

/// The event both subscribers read.
#[derive(Clone, Debug, SimpleObject)]
struct Tick {
    seq: i32,
}

/// Capacity 8, not 1: a broadcast channel drops for a *lagging* receiver.
#[nest_rs_core::injectable]
struct TickResolverState {
    tx: broadcast::Sender<Tick>,
}

impl Default for TickResolverState {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(8).0,
        }
    }
}

#[resolver]
struct TickResolver {
    #[inject]
    state: Arc<TickResolverState>,
}

#[operations]
impl TickResolver {
    #[query]
    #[public]
    async fn tick_count(&self) -> i32 {
        self.state.tx.receiver_count() as i32
    }

    #[subscription]
    #[public]
    async fn ticks(&self) -> impl futures_stream::Stream<Item = Tick> {
        let rx = self.state.tx.subscribe();
        futures_stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(tick) => Some((tick, rx)),
                Err(_) => None,
            }
        })
    }

    /// A synchronous subscription; the method the expansion emits is `async`.
    #[subscription]
    #[public]
    fn counted(&self) -> impl futures_stream::Stream<Item = i32> {
        futures_stream::iter([1, 2])
    }

    /// A per-argument pipe on a subscription, run once at subscribe.
    #[subscription]
    #[public]
    async fn labelled_ticks(
        &self,
        label: Piped<Trim, String>,
    ) -> Result<impl futures_stream::Stream<Item = String>, async_graphql::Error> {
        Ok(futures_stream::iter([label.into_inner()]))
    }

    /// Reaches for a request-scoped provider, which a socket does not carry.
    #[subscription]
    #[public]
    async fn scoped_ticks(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> Result<impl futures_stream::Stream<Item = i32>, async_graphql::Error> {
        let scoped = nest_rs_graphql::Scoped::<TickResolverState>::from_context(ctx)?;
        let count = scoped.tx.receiver_count() as i32;
        Ok(futures_stream::iter([count]))
    }
}

#[module(providers = [TickResolverState, TickResolver])]
struct TickModule;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        disable_introspection: false,
        ..GraphqlConfig::default()
    }),
    TickModule,
])]
struct SubscriptionApp;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<SubscriptionApp>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("a schema carrying a subscription boots and mounts at /graphql")
}

/// Subscribes over the graphql-ws protocol: `connection_init` →
/// `connection_ack` → `subscribe` → `next`.
#[tokio::test]
async fn a_subscriber_receives_the_items_the_resolver_emits() {
    let app = boot().await;
    let state = app
        .container()
        .get::<TickResolverState>()
        .expect("the resolver's state is a provider of the booted app");

    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("ticks", "subscription { ticks { seq } }");

    // The stream registers its receiver on the first poll; emitting before that
    // publishes to nobody.
    let emitted = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            nest_rs_testing::wait_until(Duration::from_secs(2), || state.tx.receiver_count() > 0)
                .await;
            state.tx.send(Tick { seq: 1 }).expect("a receiver is live");
            state.tx.send(Tick { seq: 2 }).expect("a receiver is live");
        }
    });

    let (first, emitted) = tokio::join!(socket.next_item("ticks"), emitted);
    emitted.expect("the emitter completes");
    let first = first.expect("the first item");
    assert_eq!(first["data"]["ticks"]["seq"], 1, "{first}");
    let second = socket.next_item("ticks").await.expect("the second item");
    assert_eq!(second["data"]["ticks"]["seq"], 2, "{second}");
}

#[tokio::test]
async fn a_synchronous_subscription_streams_like_an_async_one() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("counted", "subscription { counted }");

    let first = socket.next_item("counted").await.expect("the first item");
    assert_eq!(first["data"]["counted"], 1, "{first}");
    let second = socket.next_item("counted").await.expect("the second item");
    assert_eq!(second["data"]["counted"], 2, "{second}");
}

#[tokio::test]
async fn a_subscription_does_not_inherit_the_upgrades_request_scope() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("scoped", "subscription { scopedTicks }");

    let message = socket
        .next_item("scoped")
        .await
        .expect("the operation answers");
    let rendered = message.to_string();
    assert!(
        rendered.contains("request scope not installed"),
        "the socket reports the scope as absent rather than resolving one: {rendered}",
    );
}

#[test]
fn a_withheld_item_is_reported_with_the_operation_that_withheld_it() {
    let logs = LogCapture::install();

    let kept = nest_rs_graphql::__private::keep_masked_item("subscription ticks", Ok(Some(7)));
    assert_eq!(kept, Some(7), "a granted item is pushed unchanged");

    let refused: Option<i32> =
        nest_rs_graphql::__private::keep_masked_item("subscription ticks", Ok(None));
    assert!(refused.is_none(), "an item outside the grant is dropped");

    let failed: Option<i32> = nest_rs_graphql::__private::keep_masked_item(
        "subscription ticks",
        Err(async_graphql::Error::new("value did not reconcile")),
    );
    assert!(failed.is_none(), "a masking failure fails closed");

    let reported = logs.find("nest_rs::graphql", "subscription item withheld");
    let reasons: Vec<String> = reported
        .iter()
        .filter_map(|event| event.field("reason"))
        .collect();
    assert_eq!(
        reasons,
        vec!["not_granted", "mask_failed"],
        "both drops are traceable, and they are told apart by `reason`",
    );
    assert!(
        reported
            .iter()
            .all(|event| event.field("operation").as_deref() == Some("subscription ticks")),
        "each names the operation that withheld the item",
    );
}

#[tokio::test]
async fn a_pipe_binds_on_a_subscription_argument() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe(
        "labelled",
        "subscription { labelledTicks(label: \"  spaced  \") }",
    );

    let item = socket.next_item("labelled").await.expect("an item");
    assert_eq!(
        item["data"]["labelledTicks"], "spaced",
        "the pipe transformed the argument before the body saw it: {item}",
    );
}

/// The unannotated one does not compile: `nest-rs-macro-hygiene`'s
/// `diagnostics/graphql/subscription_without_posture.rs`.
#[tokio::test]
async fn a_public_subscription_is_reachable() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "{ __type(name: \"Subscription\") { fields { name } } }"
        }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json: serde_json::Value =
        serde_json::to_value(resp.json().await).expect("a GraphQL response is JSON");
    let fields = json["data"]["__type"]["fields"]
        .as_array()
        .expect("the schema carries a Subscription root");
    assert!(
        fields.iter().any(|f| f["name"] == "ticks"),
        "the declared subscription is in the schema: {fields:?}",
    );
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        playground: true,
        ..GraphqlConfig::default()
    }),
    TickModule,
])]
struct PlaygroundApp;

/// The completed handshake (`101`) cannot be asserted in process; the real
/// socket tests below and `demo/apps/api`'s e2e suite cover it.
#[tokio::test]
async fn an_upgrade_on_the_graphql_path_is_not_answered_by_the_playground() {
    let app = TestApp::builder()
        .module::<PlaygroundApp>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the playground app boots");

    let browser = app.http().get("/graphql").send().await;
    browser.assert_status_is_ok();
    let html = browser.0.into_body().into_string().await.expect("a body");
    assert!(
        html.contains("GraphQL"),
        "a plain GET is the playground: {html:.120}",
    );

    let upgrade = app
        .http()
        .get("/graphql")
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .send()
        .await;
    let body = upgrade
        .0
        .into_body()
        .into_string()
        .await
        .unwrap_or_default();
    assert!(
        !body.contains("GraphQL"),
        "an upgrade reaches the socket endpoint, not the playground: {body:.120}",
    );
}

#[tokio::test]
async fn a_bare_get_without_the_playground_is_a_method_error() {
    let app = boot().await;
    let resp = app.http().get("/graphql").send().await;
    resp.assert_status(poem::http::StatusCode::METHOD_NOT_ALLOWED);
}

/// `0` disables the ceiling; unset keeps the 4-hour default, as
/// `<PREFIX>_WS__MAX_CONNECTION_SECS` does.
#[test]
fn the_socket_lifetime_ceiling_defaults_on_and_is_disabled_only_by_zero() {
    use nest_rs_config::{Config, ConfigService};

    let default = GraphqlConfig::default();
    assert_eq!(
        default.max_connection,
        Some(Duration::from_secs(4 * 60 * 60)),
        "a subscription socket is bounded unless the deployment says otherwise",
    );

    let env = ConfigService::with_vars("graphql", [("MAX_CONNECTION_SECS", "0")]);
    let disabled = GraphqlConfig::from_env(&env, GraphqlConfig::default())
        .expect("`0` is the unlimited sentinel, not an error");
    assert_eq!(disabled.max_connection, None);

    let env = ConfigService::with_vars("graphql", [("MAX_CONNECTION_SECS", "30")]);
    let pinned = GraphqlConfig::from_env(&env, GraphqlConfig::default())
        .expect("a whole-second ceiling resolves");
    assert_eq!(pinned.max_connection, Some(Duration::from_secs(30)));
}

// Over a real upgrade: the close is the socket's, and the in-process driver has none.

use nest_rs_testing::ws::{WsApp, WsFrame, WsSocket, WsSocketBuilder};
use poem::web::websocket::CloseCode;

fn graphql_ws(app: &WsApp) -> WsSocketBuilder {
    app.socket("/graphql")
        .header("sec-websocket-protocol", "graphql-transport-ws")
}

/// The next text frame, parsed.
async fn next_message(socket: &mut WsSocket) -> serde_json::Value {
    match socket.next_frame().await {
        Some(WsFrame::Text(text)) => serde_json::from_str(&text).expect("a protocol message"),
        other => panic!("expected a protocol message, got {other:?}"),
    }
}

/// `connection_init`, `connection_ack`, then `subscribe` as id `1`.
async fn subscribe(socket: &mut WsSocket, query: &str) {
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(socket).await["type"], "connection_ack");
    socket
        .send_text(
            serde_json::json!({ "id": "1", "type": "subscribe", "payload": { "query": query } })
                .to_string(),
        )
        .await;
}

/// Wait until the `ticks` stream has subscribed to its source.
async fn running(state: &TickResolverState) {
    for _ in 0..250 {
        if state.tx.receiver_count() > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the subscription never started");
}

async fn ticks_app<M: nest_rs_core::Module + 'static>() -> (WsApp, Arc<TickResolverState>) {
    let app = TestApp::builder()
        .module::<M>()
        .build_ws()
        .await
        .expect("a schema carrying a subscription boots on a real port");
    let state = app
        .container()
        .get::<TickResolverState>()
        .expect("the resolver's state is a provider of the booted app");
    (app, state)
}

#[tokio::test]
async fn a_subscription_is_completed_then_closed_going_away_at_the_signal() {
    let logs = LogCapture::install();
    let (app, state) = ticks_app::<SubscriptionApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { ticks { seq } }").await;
    running(&state).await;

    let asked = std::time::Instant::now();
    let stopping = tokio::spawn(app.shutdown());
    let completed = next_message(&mut socket).await;
    let (code, reason) = socket.expect_close().await;
    stopping
        .await
        .expect("shutdown does not panic")
        .expect("the transport stops cleanly");

    assert_eq!(completed["type"], "complete", "{completed}");
    assert_eq!(completed["id"], "1", "{completed}");
    assert_eq!(
        code,
        CloseCode::Away,
        "§7.4.1 1001: the server is going down"
    );
    assert!(
        reason.contains("reconnect"),
        "the peer is told what to do: {reason}"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "a socket with nothing in flight does not spend the window, took {:?}",
        asked.elapsed(),
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::SUBSCRIPTION.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the server ended the socket, not its client",
    );
    let span = logs.expect_span(
        nest_rs_graphql::TARGET,
        nest_rs_graphql::unit::SUBSCRIPTION.name(),
    );
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
}

/// The floor every connection ceiling is held to: a second.
const CEILING: Duration = Duration::from_secs(1);

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_connection: Some(CEILING),
        ..GraphqlConfig::default()
    }),
    TickModule,
])]
struct CeilingApp;

#[tokio::test]
async fn the_lifetime_ceiling_completes_the_subscriptions_and_closes_going_away() {
    let (app, state) = ticks_app::<CeilingApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { ticks { seq } }").await;
    running(&state).await;
    // Paused time, while nothing reads: what the ceiling sends waits buffered.
    tokio::time::pause();
    tokio::time::sleep(CEILING + Duration::from_millis(1)).await;
    tokio::time::resume();

    let completed = next_message(&mut socket).await;
    let (code, reason) = socket.expect_close().await;

    assert_eq!(completed["type"], "complete", "{completed}");
    assert_eq!(completed["id"], "1", "{completed}");
    assert_eq!(code, CloseCode::Away);
    assert!(reason.contains("re-upgrade"), "{reason}");
    app.shutdown().await.expect("the transport stops cleanly");
}

#[resolver]
struct ExplodingResolver;

#[operations]
impl ExplodingResolver {
    #[subscription]
    #[public]
    async fn exploding(&self) -> impl futures_stream::Stream<Item = i32> {
        futures_stream::once(async { panic!("the subscription exploded") })
    }
}

#[module(imports = [GraphqlModule::for_root(None), TickModule], providers = [ExplodingResolver])]
struct ExplodingApp;

#[tokio::test]
async fn a_subscription_that_panics_files_panic_and_closes_with_internal_error() {
    let logs = LogCapture::install();
    let (app, _) = ticks_app::<ExplodingApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { exploding }").await;

    let (code, _) = socket.expect_close().await;

    assert_eq!(code, CloseCode::Error, "§7.4.1 1011");
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::SUBSCRIPTION.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
    );
    let contained = logs.expect_one(
        nest_rs_graphql::TARGET,
        "graphql subscription panicked; the socket is closed with an internal error",
    );
    assert_eq!(contained.level, "error");
    assert!(
        contained
            .field(nest_rs_core::panic::FIELD)
            .is_some_and(|panic| panic.contains("the subscription exploded")),
        "{contained:#?}",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

static ANSWER_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[resolver]
struct SlowAnswerResolver;

#[operations]
impl SlowAnswerResolver {
    /// Takes long enough for shutdown to be asked for while it answers, and far
    /// less than the window.
    #[query]
    #[public]
    async fn slow_answer(&self) -> i32 {
        ANSWER_STARTED.notify_one();
        tokio::time::sleep(Duration::from_millis(300)).await;
        42
    }
}

#[module(imports = [GraphqlModule::for_root(None), TickModule], providers = [SlowAnswerResolver])]
struct AnsweringApp;

#[tokio::test]
async fn a_query_answering_at_the_signal_is_answered_before_the_close() {
    let (app, state) = ticks_app::<AnsweringApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { ticks { seq } }").await;
    running(&state).await;
    socket
        .send_text(
            serde_json::json!({
                "id": "2", "type": "subscribe", "payload": { "query": "{ slowAnswer }" },
            })
            .to_string(),
        )
        .await;
    ANSWER_STARTED.notified().await;

    let stopping = tokio::spawn(app.shutdown());
    let mut seen = Vec::new();
    let (code, _) = loop {
        match socket.next_frame().await {
            Some(WsFrame::Text(text)) => {
                seen.push(serde_json::from_str::<serde_json::Value>(&text).expect("a message"));
            }
            Some(WsFrame::Close(Some(close))) => break close,
            other => panic!("expected messages then a close, got {other:?} after {seen:?}"),
        }
    };
    stopping
        .await
        .expect("shutdown does not panic")
        .expect("the transport stops cleanly");

    let of = |id: &str| -> Vec<&str> {
        seen.iter()
            .filter(|message| message["id"] == id)
            .filter_map(|message| message["type"].as_str())
            .collect()
    };
    assert_eq!(
        of("1"),
        ["complete"],
        "the subscription is completed: {seen:?}"
    );
    assert_eq!(
        of("2"),
        ["next", "complete"],
        "the query is answered: {seen:?}"
    );
    let answer = seen
        .iter()
        .find(|message| message["id"] == "2" && message["type"] == "next")
        .expect("the answer");
    assert_eq!(answer["payload"]["data"]["slowAnswer"], 42, "{answer}");
    assert_eq!(code, CloseCode::Away);
}

#[resolver]
struct FloodResolver;

#[operations]
impl FloodResolver {
    #[query]
    #[public]
    async fn flood_size(&self) -> i32 {
        64 * 1024
    }

    /// Emits as fast as it is written, more than any socket buffer holds.
    #[subscription]
    #[public]
    async fn flood(&self) -> impl futures_stream::Stream<Item = String> {
        futures_stream::repeat("x".repeat(64 * 1024))
    }
}

/// The send deadline [`StallingApp`]'s transport is served under.
const SEND_DEADLINE: Duration = Duration::from_secs(2);

#[module(
    imports = [
        nest_rs_http::HttpModule::for_root(nest_rs_http::HttpConfig {
            send_timeout: SEND_DEADLINE,
            ..nest_rs_http::HttpConfig::default()
        }),
        GraphqlModule::for_root(None),
    ],
    providers = [FloodResolver],
)]
struct StallingApp;

/// A socket whose client stops reading while a subscription pushes is dropped
/// at the send deadline, by the server: its line files `cancelled`.
#[tokio::test]
async fn a_socket_whose_client_stops_reading_is_dropped_at_the_send_deadline() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<StallingApp>()
        .build_ws()
        .await
        .expect("a schema carrying a subscription boots on a real port");
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { flood }").await;
    assert_eq!(
        next_message(&mut socket).await["type"],
        "next",
        "the flood started"
    );

    tokio::time::pause();
    tokio::time::sleep(3 * SEND_DEADLINE).await;
    tokio::time::resume();
    nest_rs_testing::wait_until(Duration::from_secs(5), || {
        !logs
            .find(
                nest_rs_core::operation_log::TARGET,
                nest_rs_graphql::unit::SUBSCRIPTION.name(),
            )
            .is_empty()
    })
    .await;

    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::SUBSCRIPTION.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the server dropped the socket, not its client",
    );
    let stalled = logs.expect_one(
        nest_rs_graphql::TARGET,
        "the peer took nothing within the send deadline; it is cut off",
    );
    assert_eq!(stalled.level, "warn");
    app.shutdown().await.expect("the transport stops cleanly");
    drop(socket);
}
