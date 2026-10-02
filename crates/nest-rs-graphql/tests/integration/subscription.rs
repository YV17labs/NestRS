//! `#[subscription]`: the third discovered root, the graphql-ws mount, and the
//! socket-lifetime ceiling that bounds it.
//!
//! The composition witness is `a_subscriber_receives_the_items_the_resolver_emits`:
//! it boots the documented wiring through [`TestApp`], subscribes, emits, and
//! asserts what the subscriber received — executed, not merely compiled.
//! Posture *per item* is `nest-rs-authz`'s witness
//! (`tests/integration/graphql/mask.rs`), where the entity fixtures live.

use std::sync::Arc;
use std::time::Duration;

use async_graphql::SimpleObject;
/// async-graphql's own `futures_util` re-export, reached through the framework
/// so this test declares no stream crate of its own — the same rooting rule the
/// decorators follow.
use async_graphql::futures_util::stream as futures_stream;
use nest_rs_core::module;
use nest_rs_graphql::async_graphql;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_pipes::{Piped, Trim};
use nest_rs_testing::{LogCapture, TestApp};
use tokio::sync::broadcast;

/// The event both subscribers read. One source, so a difference in what two
/// callers receive can only come from the posture.
#[derive(Clone, Debug, SimpleObject)]
struct Tick {
    seq: i32,
}

/// Capacity is 8 rather than 1: a broadcast channel drops for a *lagging*
/// receiver, and a test that raced the lag path would fail for a reason that has
/// nothing to do with what it asserts.
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

    /// A synchronous subscription. The method the root publishes is the one
    /// the expansion emits, and that one is `async`; what the developer writes
    /// only decides whether it is awaited.
    #[subscription]
    #[public]
    fn counted(&self) -> impl futures_stream::Stream<Item = i32> {
        futures_stream::iter([1, 2])
    }

    /// A per-argument pipe on a subscription: the wire exposes `label`, the pipe
    /// runs once at subscribe, and the stream carries the transformed value.
    #[subscription]
    #[public]
    async fn labelled_ticks(
        &self,
        label: Piped<Trim, String>,
    ) -> Result<impl futures_stream::Stream<Item = String>, async_graphql::Error> {
        Ok(futures_stream::iter([label.into_inner()]))
    }

    /// Reaches for a request-scoped provider, which a socket deliberately does
    /// not carry — the operation must say so rather than resolve one.
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

/// The composition witness. Boots the documented wiring, subscribes over the
/// **graphql-ws protocol** the mount serves — `connection_init` →
/// `connection_ack` → `subscribe` → `next` — emits, and asserts what the
/// subscriber received.
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

    // The stream registers its receiver on the first poll, which the driver
    // makes while waiting for a message; emitting before that publishes into a
    // channel nobody is listening on.
    let emitted = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            while state.tx.receiver_count() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            state.tx.send(Tick { seq: 1 }).expect("a receiver is live");
            state.tx.send(Tick { seq: 2 }).expect("a receiver is live");
        }
    });

    let first = socket.next_item("ticks").await.expect("the first item");
    assert_eq!(first["data"]["ticks"]["seq"], 1, "{first}");
    let second = socket.next_item("ticks").await.expect("the second item");
    assert_eq!(second["data"]["ticks"]["seq"], 2, "{second}");

    emitted.await.expect("the emitter completes");
}

/// `fn` and `async fn` are both accepted at every impl half, a subscription
/// included — the async-ness async-graphql requires is the emitted method's.
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

/// A request-scoped provider is **not** the connection's. The upgrade's
/// `RequestScope` stops at the upgrade, so a subscription reaching `Scoped<T>`
/// is told the scope is absent rather than handed an instance built once, at
/// connect, and shared by every operation for the socket's life.
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

/// A dropped item is the one thing on this path that leaves no trace on the
/// wire — the stream simply skips it. So the trace has to be in the log, with
/// enough on it to find the operation, or an operator debugging "my subscriber
/// misses events" has nothing at all.
#[test]
fn a_withheld_item_is_reported_with_the_operation_that_withheld_it() {
    let logs = LogCapture::install();

    let kept = nest_rs_graphql::keep_masked_item("subscription ticks", Ok(Some(7)));
    assert_eq!(kept, Some(7), "a granted item is pushed unchanged");

    let refused: Option<i32> = nest_rs_graphql::keep_masked_item("subscription ticks", Ok(None));
    assert!(refused.is_none(), "an item outside the grant is dropped");

    let failed: Option<i32> = nest_rs_graphql::keep_masked_item(
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

/// Per-argument pipes bind on a subscription exactly as on a query — the wire
/// value goes in, the carrier reaches the body, and the pipe runs **once**, at
/// subscribe, not per item.
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

/// A `#[public]` subscription is reachable — the posture's other half. (The
/// unannotated one does not compile: see
/// `tests/integration/diagnostics/subscription_without_posture.rs`.)
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

/// `GET <path>` serves two things, and the request decides which. This is the
/// dispatcher's own seam: a browser gets the playground, an upgrade gets the
/// socket — one URL, which is what every graphql-ws client assumes.
///
/// The completed handshake (`101`) cannot be asserted here: `TestClient` runs
/// the endpoint in-process, where there is no connection to upgrade. That half
/// is proven over a real socket by `demo/apps/api`'s e2e suite.
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

/// With the playground off — the production default — the path serves POST and
/// sockets only, so a bare GET is the wrong method rather than a missing route.
#[tokio::test]
async fn a_bare_get_without_the_playground_is_a_method_error() {
    let app = boot().await;
    let resp = app.http().get("/graphql").send().await;
    resp.assert_status(poem::http::StatusCode::METHOD_NOT_ALLOWED);
}

/// The ceiling is a security control, so its "off" spelling has to be the
/// deliberate one. `0` disables it; unset keeps the 4-hour default — the same
/// three cases `NESTRS_WS__MAX_CONNECTION_SECS` carries.
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

// ── The way down ────────────────────────────────────────────────────────────
//
// The socket is a connection poem stops tracking at the upgrade, so the
// shutdown window neither waited for one nor closed it: every subscription ran
// on under the shutdown hooks until the process exit cut it — and the lifetime
// ceiling dropped the socket outright. Both now end the protocol's way: every
// running subscription is completed, then the socket closes with RFC 6455
// §7.4.1's 1001 Going Away. These run over a real upgrade, because the close is
// the socket's and the in-process driver has none.

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

/// Wait until the `ticks` stream has subscribed to its source — the operation is
/// running from then on.
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

/// At the signal a subscription is told it is over with the protocol's own
/// `complete` — the server's end of an operation that is not an error — and the
/// socket then closes with 1001 and a reason saying what to do. The transport
/// waits for that close rather than returning past a socket it never told, and
/// the socket's line says the server ended it.
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
        nest_rs_graphql::unit::SUBSCRIPTION,
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the server ended the socket, not its client",
    );
    // The socket's span fails with the line's word.
    let span = logs.expect_span(nest_rs_graphql::TARGET, nest_rs_graphql::unit::SUBSCRIPTION);
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_connection: Some(Duration::from_millis(300)),
        ..GraphqlConfig::default()
    }),
    TickModule,
])]
struct CeilingApp;

/// The ceiling ends a socket the same way: it forces a re-upgrade so the guard
/// runs again, and a client that read the 1006 a dropped socket gave it could
/// not tell that from a network fault, nor which of its subscriptions had ended.
#[tokio::test]
async fn the_lifetime_ceiling_completes_the_subscriptions_and_closes_going_away() {
    let (app, state) = ticks_app::<CeilingApp>().await;
    let mut socket = graphql_ws(&app).connect().await;
    subscribe(&mut socket, "subscription { ticks { seq } }").await;
    running(&state).await;

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

/// A subscription stream that panics took the socket task down: no line, and a
/// socket the peer read as 1006. The socket is the unit here, so it files
/// `panic` and closes with §7.4.1's 1011 Internal Error.
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
        nest_rs_graphql::unit::SUBSCRIPTION,
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
            .field("panic")
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

/// A query sent over the socket is a unit still answering, not a channel
/// without an end: at the signal it is answered, inside the window as a request
/// running over HTTP is — while the subscription beside it is completed at
/// once — and only then does the socket close.
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
