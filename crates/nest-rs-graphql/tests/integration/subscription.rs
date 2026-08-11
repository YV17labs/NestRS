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

    let env = ConfigService::with_vars("graphql", [("NESTRS_GRAPHQL__MAX_CONNECTION_SECS", "0")]);
    let disabled = GraphqlConfig::from_env(&env, GraphqlConfig::default())
        .expect("`0` is the unlimited sentinel, not an error");
    assert_eq!(disabled.max_connection, None);

    let env = ConfigService::with_vars("graphql", [("NESTRS_GRAPHQL__MAX_CONNECTION_SECS", "30")]);
    let pinned = GraphqlConfig::from_env(&env, GraphqlConfig::default())
        .expect("a whole-second ceiling resolves");
    assert_eq!(pinned.max_connection, Some(Duration::from_secs(30)));
}
