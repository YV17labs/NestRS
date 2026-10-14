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
    socket.send_text(subscribe_message("1", query)).await;
}

/// The `subscribe` message for `query` as operation `id`.
fn subscribe_message(id: &str, query: &str) -> String {
    serde_json::json!({ "id": id, "type": "subscribe", "payload": { "query": query } }).to_string()
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

/// A frame that is not a graphql-ws message closes the socket `1002` with a
/// fixed reason, within what a close frame carries: serde's sentence would
/// quote the payload.
#[tokio::test]
async fn a_frame_that_does_not_decode_closes_the_socket_without_quoting_it() {
    let (app, _) = ticks_app::<SubscriptionApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");

    socket
        .send_text(r#"{"type":"subscribe","id":"1","payload":"sk_live_secret"}"#)
        .await;
    let (code, reason) = socket.expect_close().await;

    assert_eq!(u16::from(code), 1002, "§7.4.1 1002: a protocol error");
    assert!(!reason.contains("sk_live"), "{reason}");
    assert!(reason.len() <= 123, "{}", reason.len());
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

/// Who the upgrade was for, read back by the operations its socket runs.
#[derive(Debug)]
struct Caller(&'static str);

impl nest_rs_core::AmbientValue for Caller {}

/// Adds the caller around the upgrade request, as a guard's attachment does.
#[nest_rs_core::injectable]
#[derive(Default)]
struct CallerInterceptor;

impl nest_rs_core::Layer for CallerInterceptor {}

#[nest_rs_http::async_trait]
impl nest_rs_interceptors::Interceptor for CallerInterceptor {
    async fn intercept(
        &self,
        req: poem::Request,
        next: nest_rs_interceptors::Next<'_>,
    ) -> poem::Result<poem::Response> {
        nest_rs_core::with_ambient(Arc::new(Caller("alice")), next.run(req)).await
    }
}

#[resolver]
struct CallerResolver;

#[operations]
impl CallerResolver {
    #[query]
    #[public]
    async fn caller_ping(&self) -> bool {
        true
    }

    #[subscription]
    #[public]
    async fn caller(&self) -> impl futures_stream::Stream<Item = String> {
        let caller = nest_rs_core::ambient::<Caller>().map_or("nobody", |caller| caller.0);
        futures_stream::iter([caller.to_owned()])
    }

    /// Whether the operation reads a request scope off the ambient context, as
    /// a service resolving a request-scoped provider would.
    #[subscription]
    #[public]
    async fn request_scoped(&self) -> impl futures_stream::Stream<Item = bool> {
        let scoped = nest_rs_core::current_request_scope().is_some();
        futures_stream::iter([scoped])
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [CallerInterceptor, CallerResolver],
)]
struct CallerApp;

#[tokio::test]
async fn an_operation_on_the_socket_reads_what_its_upgrade_carried() {
    let app = TestApp::builder()
        .module::<CallerApp>()
        .use_interceptors_global([nest_rs_interceptors::interceptor::<CallerInterceptor>()])
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { caller }").await;

    let item = next_message(&mut socket).await;
    assert_eq!(item["type"], "next", "{item}");
    assert_eq!(item["payload"]["data"]["caller"], "alice", "{item}");
}

#[tokio::test]
async fn an_operation_on_the_socket_reads_no_request_scope_off_its_upgrade() {
    let app = TestApp::builder()
        .module::<CallerApp>()
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { requestScoped }").await;

    let item = next_message(&mut socket).await;
    assert_eq!(item["type"], "next", "{item}");
    assert_eq!(
        item["payload"]["data"]["requestScoped"], false,
        "the upgrade ran with a request scope, and the socket carries none of it: {item}",
    );
}

/// A page on another site, which no CORS policy below lists.
const ELSEWHERE: &str = "https://evil.example";

/// The browser app a CORS policy lists.
const LISTED: &str = "https://app.example";

/// RFC 6455 §10.2: CORS never asks the browser about an upgrade, so a page on
/// another site could open a socket under the visitor's cookies.
#[tokio::test]
async fn a_browser_upgrade_from_another_origin_is_refused_while_cors_is_off() {
    let logs = LogCapture::install();
    let (app, _) = ticks_app::<SubscriptionApp>().await;

    let status = graphql_ws(&app)
        .header("origin", ELSEWHERE)
        .expect_refused()
        .await;

    assert_eq!(status, poem::http::StatusCode::FORBIDDEN);
    let refused = logs.expect_one(nest_rs_graphql::TARGET, "browser origin refused");
    assert_eq!(refused.level, "warn");
    assert_eq!(refused.field("origin").as_deref(), Some(ELSEWHERE));
    assert_eq!(
        refused.field("expected"),
        Some(format!("http://{}", app.addr()))
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn an_upgrade_with_no_origin_or_its_own_is_served() {
    let (app, _) = ticks_app::<SubscriptionApp>().await;

    for origin in [None, Some(format!("http://{}", app.addr()))] {
        let builder = graphql_ws(&app);
        let builder = match origin {
            Some(origin) => builder.header("origin", origin),
            None => builder,
        };
        let mut socket = builder.connect().await;
        socket.send_text(r#"{"type":"connection_init"}"#).await;
        assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    }

    app.shutdown().await.expect("the transport stops cleanly");
}

#[module(imports = [
    nest_rs_http::HttpModule::for_root(nest_rs_http::HttpConfig {
        cors: Some(nest_rs_http::HttpCors {
            origins: vec![LISTED.to_owned()],
            ..nest_rs_http::HttpCors::default()
        }),
        ..nest_rs_http::HttpConfig::default()
    }),
    GraphqlModule::for_root(None),
    TickModule,
])]
struct ListedOriginApp;

/// Once CORS is set, its layer answers first and admits what it lists alone.
#[tokio::test]
async fn once_cors_is_set_only_the_origins_it_lists_open_a_socket() {
    let logs = LogCapture::install();
    let (app, _) = ticks_app::<ListedOriginApp>().await;

    let mut socket = graphql_ws(&app).header("origin", LISTED).connect().await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    assert_eq!(
        graphql_ws(&app)
            .header("origin", ELSEWHERE)
            .expect_refused()
            .await,
        poem::http::StatusCode::FORBIDDEN,
    );
    assert_eq!(
        graphql_ws(&app)
            .header("origin", format!("http://{}", app.addr()))
            .expect_refused()
            .await,
        poem::http::StatusCode::FORBIDDEN,
        "the server's own origin, unlisted",
    );
    logs.expect_none(nest_rs_graphql::TARGET, "browser origin refused");

    app.shutdown().await.expect("the transport stops cleanly");
}

/// A frame past the socket's message cap is never buffered whole: the protocol
/// layer refuses it and the socket closes. 1011, not 1009: poem hands the cause
/// on as an opaque `io::Error`, as the WebSocket gateway's cap meets it.
#[tokio::test]
async fn a_frame_past_the_message_cap_closes_the_socket() {
    let logs = LogCapture::install();
    let (app, _) = ticks_app::<SubscriptionApp>().await;
    let mut socket = graphql_ws(&app).connect().await;

    let padding = "x".repeat(128 * 1024);
    socket
        .send_text(
            serde_json::json!({ "type": "connection_init", "payload": { "padding": padding } })
                .to_string(),
        )
        .await;
    let (code, reason) = socket.expect_close().await;

    assert_eq!(code, CloseCode::Error, "{reason}");
    assert!(!reason.is_empty());
    let read = logs.expect_one(nest_rs_graphql::TARGET, "subscription socket read failed");
    assert!(
        read.field("error")
            .is_some_and(|error| error.contains("Space limit")),
        "the operator's line names the size that did it: {:?}",
        read.fields,
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        connection_init_timeout: Duration::from_secs(1),
        ..GraphqlConfig::default()
    }),
    TickModule,
])]
struct InitTimeoutApp;

/// graphql-ws `PROTOCOL.md`, *ConnectionInit*: a socket that has not sent
/// `connection_init` within the wait is closed `4408`.
#[tokio::test]
async fn a_socket_that_sends_no_connection_init_is_closed_4408() {
    let (app, _) = ticks_app::<InitTimeoutApp>().await;
    let mut socket = graphql_ws(&app).connect().await;

    // Paused time, while nothing reads: the Close waits buffered.
    tokio::time::pause();
    tokio::time::sleep(Duration::from_secs(1) + Duration::from_millis(1)).await;
    tokio::time::resume();
    let (code, reason) = socket.expect_close().await;

    assert_eq!(u16::from(code), 4408);
    assert_eq!(reason, "Connection initialisation timeout");
    app.shutdown().await.expect("the transport stops cleanly");
}

/// graphql-ws `PROTOCOL.md`, *Subscribe*: an id already running closes the
/// socket `4409`, rather than silently replacing the running operation.
#[tokio::test]
async fn a_reused_operation_id_closes_the_socket_4409_without_echoing_it() {
    let (app, state) = ticks_app::<SubscriptionApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { ticks { seq } }").await;
    running(&state).await;

    socket
        .send_text(subscribe_message("1", "subscription { ticks { seq } }"))
        .await;
    let (code, reason) = socket.expect_close().await;

    assert_eq!(u16::from(code), 4409);
    assert!(!reason.contains('1'), "the id is not echoed: {reason}");
    app.shutdown().await.expect("the transport stops cleanly");
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_socket_operations: Some(2),
        ..GraphqlConfig::default()
    }),
    TickModule,
])]
struct OperationCapApp;

#[tokio::test]
async fn an_operation_past_the_socket_cap_is_answered_an_error_and_the_rest_keep_streaming() {
    let (app, state) = ticks_app::<OperationCapApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    for id in ["1", "2", "3"] {
        socket
            .send_text(subscribe_message(id, "subscription { ticks { seq } }"))
            .await;
    }

    let refused = next_message(&mut socket).await;
    assert_eq!(refused["type"], "error", "{refused}");
    assert_eq!(refused["id"], "3", "{refused}");
    assert_eq!(
        refused["payload"][0]["message"], "too many operations on this socket",
        "{refused}"
    );
    nest_rs_testing::wait_until(Duration::from_secs(5), || state.tx.receiver_count() == 2).await;
    state
        .tx
        .send(Tick { seq: 7 })
        .expect("two subscribers listen");
    let mut streamed: Vec<String> = Vec::new();
    for _ in 0..2 {
        let item = next_message(&mut socket).await;
        assert_eq!(item["type"], "next", "{item}");
        streamed.push(item["id"].as_str().expect("an id").to_owned());
    }
    streamed.sort();
    assert_eq!(streamed, ["1", "2"]);
    assert_eq!(state.tx.receiver_count(), 2, "the third never ran");
    app.shutdown().await.expect("the transport stops cleanly");
}
