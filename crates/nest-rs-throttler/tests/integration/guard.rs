//! [`ThrottlerGuard`] bound **globally** reads a route's `#[meta(Throttle)]`
//! (`src/guard.rs`), and bounds its store by `HIT_TIMEOUT` on every edge.
//!
//! The module default is pinned generously (60/minute), so a `429` on the third
//! request can only come from the route's own declaration. The stalled stores
//! run on paused time, so the twenty seconds cost nothing.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering, module};
use nest_rs_guards::guard;
use nest_rs_http::{HttpConfig, HttpModule, async_trait, controller, routes};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_throttler::{
    BACKEND_REMEDY, Decision, HIT_TIMEOUT, Throttle, ThrottlerConfig, ThrottlerGuard,
    ThrottlerModule, ThrottlerStore,
};
use poem::http::StatusCode;

#[controller(path = "/rated")]
struct RatedController;

// No `#[use_guards]` at either scope: the pool alone reaches these routes.
#[routes]
impl RatedController {
    /// Two per minute, declared on the route and nowhere else.
    #[get("/strict")]
    #[meta(Throttle::per_minute(2))]
    async fn strict(&self) -> &'static str {
        "ok"
    }

    /// No `#[meta]` — the module default applies.
    #[get("/lenient")]
    async fn lenient(&self) -> &'static str {
        "ok"
    }

    /// Two per minute across every item: one route, one bucket.
    #[get("/items/{id}")]
    #[meta(Throttle::per_minute(2))]
    async fn item(&self) -> &'static str {
        "ok"
    }
}

#[module(
    imports = [
        ThrottlerModule::for_root(ThrottlerConfig {
            limit: Some(60),
            window_secs: Some(60),
            pseudonym_key: None,
        }),
    ],
    providers = [RatedController],
)]
struct RatedModule;

/// The key the suite's stores outside the process count under — a fixture,
/// never a secret.
const PSEUDONYM_KEY: &str = "nest-rs-throttler integration pseudonym key";

/// The documented global wiring: the throttler in the app's imports, the guard
/// in `use_guards_global`, nothing on the controller.
async fn app() -> TestApp {
    TestApp::builder()
        .module::<RatedModule>()
        .use_guards_global([guard::<ThrottlerGuard>()])
        .build()
        .await
        .expect("importing ThrottlerModule provides the guard the pool names")
}

#[tokio::test]
async fn a_pooled_guard_reads_the_route_s_throttle_metadata() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = app().await;

    app.http()
        .get("/rated/strict")
        .send()
        .await
        .assert_status_is_ok();
    app.http()
        .get("/rated/strict")
        .send()
        .await
        .assert_status_is_ok();

    // A `200` here would mean the pool ran with no route metadata attached.
    let denied = app.http().get("/rated/strict").send().await;
    denied.assert_status(StatusCode::TOO_MANY_REQUESTS);
    denied.assert_header_exist("retry-after");

    // The route, never the composite store key, which holds the client's address.
    let event = logs.expect_one(nest_rs_throttler::TARGET, "rate limit exceeded");
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("route")
            .is_some_and(|r| r.contains("/rated/strict")),
        "the event names the route on its own field, got {:?}",
        event.fields,
    );
    assert_eq!(
        event.field("client"),
        None,
        "the event names no client address, got {:?}",
        event.fields,
    );
    assert!(
        event.field("retry_after").is_some(),
        "the event carries the wait it told the client about, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn the_bucket_key_is_the_declared_template() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = app().await;

    for item in ["/rated/items/1", "/rated/items/2"] {
        app.http().get(item).send().await.assert_status_is_ok();
    }
    // A third item, never asked for: the budget is the route's, not the path's.
    app.http()
        .get("/rated/items/3")
        .send()
        .await
        .assert_status(StatusCode::TOO_MANY_REQUESTS);

    let event = logs.expect_one(nest_rs_throttler::TARGET, "rate limit exceeded");
    assert_eq!(
        event.field("route").as_deref(),
        Some("/rated/items/{id}"),
        "the bucket is named as the route declares it, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_route_with_no_metadata_falls_back_to_the_module_default() {
    let app = app().await;

    // What makes the sibling test's `429` attributable to the metadata rather
    // than to a low default the deployment supplied.
    for _ in 0..3 {
        app.http()
            .get("/rated/lenient")
            .send()
            .await
            .assert_status_is_ok();
    }
}

/// A store that never answers — a backend holding every command, a network
/// dropping them without a reset.
#[derive(Default)]
struct StalledStore;

#[async_trait]
impl ThrottlerStore for StalledStore {
    async fn hit(&self, _key: &str, _limit: Throttle) -> Decision {
        std::future::pending().await
    }
}

/// A store that answers just inside the guard's bound, as a store running out
/// its own budget does.
#[derive(Default)]
struct SlowStore;

#[async_trait]
impl ThrottlerStore for SlowStore {
    async fn hit(&self, _key: &str, _limit: Throttle) -> Decision {
        tokio::time::sleep(HIT_TIMEOUT - Duration::from_millis(1)).await;
        Decision::allowed()
    }
}

/// Binds `S` the way a store's own module does: a declared factory carrying the
/// port's remedy.
struct StoreModule<S>(std::marker::PhantomData<S>);

impl<S: ThrottlerStore + Default> Module for StoreModule<S> {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_declared_factory::<Arc<dyn ThrottlerStore>, _, _>(BACKEND_REMEDY, |_| async {
                Ok(Arc::new(S::default()) as Arc<dyn ThrottlerStore>)
            })
    }
}

type StalledStoreModule = StoreModule<StalledStore>;
type SlowStoreModule = StoreModule<SlowStore>;

/// The line says what the refusal cannot: that the store never answered, and
/// which store it was.
fn assert_the_store_was_waited_out(logs: &LogCapture, transport: &str) {
    let cause = logs.expect_one(
        nest_rs_throttler::TARGET,
        "throttler store did not answer within the guard's timeout; denying (fail-closed)",
    );
    assert_eq!(cause.level, "warn");
    assert_eq!(cause.field("transport").as_deref(), Some(transport));
    assert_eq!(
        cause.field("store").as_deref(),
        Some(std::any::type_name::<StalledStore>()),
        "the line names the store behind the trait object, got {:?}",
        cause.fields,
    );
    assert_eq!(
        cause.field("waited_ms").as_deref(),
        Some(HIT_TIMEOUT.as_millis().to_string().as_str()),
    );
}

/// The window the stalled-store apps pin, and so the `Retry-After` a refusal for
/// a store that cannot answer carries.
const WINDOW_SECS: u64 = 60;

/// The HTTP edge the stalled-store apps mount, with its request timeout at the
/// default — the budget the guard's bound has to answer inside.
fn edge() -> HttpConfig {
    HttpConfig {
        port: 0,
        ..Default::default()
    }
}

/// Await `unit` on the paused clock for twice the bound and no longer, so a
/// guard that stopped bounding its store fails here rather than hang.
async fn within_twice_the_bound<T>(unit: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(HIT_TIMEOUT * 2, unit)
        .await
        .unwrap_or_else(|_| panic!("no answer within twice HIT_TIMEOUT ({HIT_TIMEOUT:?})"))
}

#[controller(path = "/stalled")]
#[use_guards(ThrottlerGuard)]
struct StalledController;

#[routes]
impl StalledController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(
    imports = [
        HttpModule::for_root(edge()),
        ThrottlerModule::for_root(ThrottlerConfig {
            limit: Some(60),
            window_secs: Some(WINDOW_SECS),
            pseudonym_key: Some(PSEUDONYM_KEY.to_owned()),
        }),
        StalledStoreModule,
    ],
    providers = [StalledController],
)]
struct StalledHttpModule;

#[module(
    imports = [
        HttpModule::for_root(edge()),
        ThrottlerModule::for_root(ThrottlerConfig {
            limit: Some(60),
            window_secs: Some(WINDOW_SECS),
            pseudonym_key: Some(PSEUDONYM_KEY.to_owned()),
        }),
        SlowStoreModule,
    ],
    providers = [StalledController],
)]
struct SlowHttpModule;

#[tokio::test(start_paused = true)]
async fn a_store_that_never_answers_is_refused_at_the_bound_over_http() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<StalledHttpModule>()
        .await
        .expect("a custom store bound the documented way boots");

    let sent = tokio::time::Instant::now();
    let refused = within_twice_the_bound(app.http().get("/stalled/ping").send()).await;
    let waited = sent.elapsed();

    // A `503` here is the edge's request timeout answering first: the guard's
    // bound would be past it.
    refused.assert_status(StatusCode::TOO_MANY_REQUESTS);
    refused.assert_header("retry-after", WINDOW_SECS.to_string());
    assert!(
        waited >= HIT_TIMEOUT,
        "refused at the bound, not before: {waited:?}"
    );
    let timeout = edge()
        .request_timeout
        .expect("the HTTP edge bounds a request by default");
    assert!(
        waited < timeout,
        "…and before the edge's own request timeout: {waited:?}"
    );
    assert_the_store_was_waited_out(&logs, "http");
}

#[tokio::test(start_paused = true)]
async fn a_store_answering_inside_the_bound_is_waited_for() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<SlowHttpModule>()
        .await
        .expect("a custom store bound the documented way boots");

    within_twice_the_bound(app.http().get("/stalled/ping").send())
        .await
        .assert_status_is_ok();
    logs.expect_none(
        nest_rs_throttler::TARGET,
        "throttler store did not answer within the guard's timeout; denying (fail-closed)",
    );
}

#[test]
fn the_bound_answers_before_the_http_edge_times_a_request_out() {
    let timeout = HttpConfig::default()
        .request_timeout
        .expect("the HTTP edge bounds a request by default");
    assert!(HIT_TIMEOUT < timeout, "{HIT_TIMEOUT:?} vs {timeout:?}");
}

// Each module below binds the guard where a developer would: the only witness
// its capability marker has, since `#[operations]`, `#[tools]` and `#[messages]`
// bound it against `GraphqlGuard` / `McpGuard` / `WsGuard`.

/// One request per minute, so the *second* call is the assertion and the first
/// proves the site was reachable at all.
#[cfg(any(feature = "graphql", feature = "mcp", feature = "ws"))]
fn one_per_minute() -> ThrottlerConfig {
    ThrottlerConfig {
        limit: Some(1),
        window_secs: Some(60),
        pseudonym_key: Some(PSEUDONYM_KEY.to_owned()),
    }
}

#[cfg(feature = "graphql")]
mod graphql {
    use nest_rs_core::module;
    use nest_rs_graphql::async_graphql::Result as GraphqlResult;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_testing::TestApp;
    use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule};

    use super::{
        StalledStoreModule, assert_the_store_was_waited_out, edge, one_per_minute,
        within_twice_the_bound,
    };

    #[resolver]
    #[use_guards(ThrottlerGuard)]
    struct RatedResolver;

    #[operations]
    impl RatedResolver {
        #[query]
        #[public]
        async fn tick(&self) -> GraphqlResult<String> {
            Ok("ok".to_owned())
        }
    }

    #[module(
        imports = [
            GraphqlModule::for_root(None),
            ThrottlerModule::for_root(one_per_minute()),
        ],
        providers = [RatedResolver],
    )]
    struct RatedGraphqlModule;

    #[module(
        imports = [
            nest_rs_http::HttpModule::for_root(edge()),
            GraphqlModule::for_root(None),
            ThrottlerModule::for_root(one_per_minute()),
            StalledStoreModule,
        ],
        providers = [RatedResolver],
    )]
    struct StalledGraphqlModule;

    async fn app() -> TestApp {
        TestApp::for_module::<RatedGraphqlModule>()
            .await
            .expect("a resolver binding ThrottlerGuard boots")
    }

    async fn tick(app: &TestApp) -> String {
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ tick }" }))
            .send()
            .await
            .0
            .into_body()
            .into_string()
            .await
            .expect("the endpoint answers with a body")
    }

    #[tokio::test]
    async fn a_second_operation_inside_the_window_is_refused() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = app().await;

        let first = tick(&app).await;
        assert!(
            first.contains("\"tick\":\"ok\""),
            "the field resolves inside the budget: {first}",
        );

        let second = tick(&app).await;
        assert!(
            second.contains("Too Many Requests"),
            "the second operation inside the window is refused: {second}",
        );
        assert!(
            !second.contains("\"tick\":\"ok\""),
            "…and the resolver body never ran: {second}",
        );

        let event = logs.expect_one(nest_rs_throttler::TARGET, "rate limit exceeded");
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("transport").as_deref(),
            Some("graphql"),
            "the family shares one message, so the edge is a field an operator \
             filters on, got {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("operation").as_deref(),
            Some("tick"),
            "the bucket is the field's, so the field is what the line names, \
             got {:?}",
            event.fields,
        );
        assert!(
            event.field("retry_after").is_some(),
            "the event carries the wait, got {:?}",
            event.fields,
        );

        // Only for an anonymous caller, which this fixture is.
        let degraded = logs.expect_one(
            nest_rs_throttler::TARGET,
            "rate-limit keying degraded to a shared bucket",
        );
        assert_eq!(degraded.level, "warn");
        assert_eq!(
            degraded.field("reason").as_deref(),
            Some("graphql_anonymous_operation_shares_a_bucket"),
        );
    }

    /// A query's, which the HTTP edge would otherwise have answered with a `503`;
    /// a subscription's fields have no such timeout at all.
    #[tokio::test(start_paused = true)]
    async fn a_store_that_never_answers_refuses_the_field_at_the_bound() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<StalledGraphqlModule>()
            .await
            .expect("a resolver over a custom store boots");

        let refused = within_twice_the_bound(tick(&app)).await;

        assert!(
            refused.contains("Too Many Requests"),
            "a store that cannot answer refuses the field: {refused}",
        );
        assert!(
            !refused.contains("\"tick\":\"ok\""),
            "…and the resolver body never ran: {refused}",
        );
        assert_the_store_was_waited_out(&logs, "graphql");
    }
}

#[cfg(feature = "mcp")]
mod mcp {
    use nest_rs_core::module;
    use nest_rs_mcp::{AllowAllMcpGuard, McpError, McpOperationGuard, mcp, tools};
    use nest_rs_testing::TestApp;
    use nest_rs_testing::mcp::call_tool;
    use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule};

    use super::{
        StalledStoreModule, assert_the_store_was_waited_out, edge, one_per_minute,
        within_twice_the_bound,
    };

    const PATH: &str = "/mcp/rated";

    #[mcp(path = "/mcp/rated")]
    #[use_guards(ThrottlerGuard)]
    #[derive(Clone, Default)]
    struct RatedTool;

    #[tools]
    impl RatedTool {
        /// Answer with a constant — the assertions are about what ran around it.
        #[tool]
        #[public]
        async fn tick(&self) -> Result<String, McpError> {
            Ok("ok".to_owned())
        }
    }

    // Without an operation guard `/mcp` is deny-all, and a `401` would hide
    // whether the per-operation chain ran at all.
    #[module(
        imports = [ThrottlerModule::for_root(one_per_minute())],
        providers = [RatedTool, AllowAllMcpGuard as dyn McpOperationGuard],
    )]
    struct RatedMcpModule;

    // `sse-stream` re-arms its keep-alive from the real clock, which a paused one
    // leaves behind at every jump.
    #[module(
        imports = [
            nest_rs_http::HttpModule::for_root(edge()),
            nest_rs_mcp::McpModule::for_root(nest_rs_mcp::McpConfig {
                sse_keep_alive: None,
                ..Default::default()
            }),
            ThrottlerModule::for_root(one_per_minute()),
            StalledStoreModule,
        ],
        providers = [RatedTool, AllowAllMcpGuard as dyn McpOperationGuard],
    )]
    struct StalledMcpModule;

    #[tokio::test]
    async fn a_second_tool_call_inside_the_window_is_refused() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<RatedMcpModule>()
            .await
            .expect("an #[mcp] host binding ThrottlerGuard boots");

        let first = call_tool(app.http(), PATH, "tick", None).await;
        assert!(
            first.contains("ok"),
            "the tool runs inside the budget: {first}"
        );

        let second = call_tool(app.http(), PATH, "tick", None).await;
        assert!(
            second.contains("Too Many Requests"),
            "the second call inside the window is refused: {second}",
        );

        let event = logs.expect_one(nest_rs_throttler::TARGET, "rate limit exceeded");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("transport").as_deref(), Some("mcp"));
        assert_eq!(
            event.field("operation").as_deref(),
            Some("tick"),
            "the bucket is the operation's, got {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("kind").as_deref(),
            Some("tool"),
            "…and the kind beside it, because a tool and a prompt may share a \
             name and are two addresses, got {:?}",
            event.fields,
        );
        assert!(event.field("retry_after").is_some());

        let degraded = logs.expect_one(
            nest_rs_throttler::TARGET,
            "rate-limit keying degraded to a shared bucket",
        );
        assert_eq!(degraded.level, "warn");
        assert_eq!(
            degraded.field("reason").as_deref(),
            Some("mcp_anonymous_operation_shares_a_bucket"),
        );
    }

    /// The call answers over a stream, after the handler the HTTP request timeout
    /// covers has returned: the guard's bound is the only one the operation has.
    #[tokio::test(start_paused = true)]
    async fn a_store_that_never_answers_refuses_the_tool_call_at_the_bound() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<StalledMcpModule>()
            .await
            .expect("an #[mcp] host over a custom store boots");

        let refused = within_twice_the_bound(call_tool(app.http(), PATH, "tick", None)).await;

        assert!(
            refused.contains("Too Many Requests"),
            "a store that cannot answer refuses the call: {refused}",
        );
        assert_the_store_was_waited_out(&logs, "mcp");
    }
}

#[cfg(feature = "ws")]
mod ws {
    use nest_rs_core::module;
    use nest_rs_guards::Guard;
    use nest_rs_testing::TestApp;
    use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule};
    use nest_rs_ws::{WsClient, WsModule, gateway, messages};

    use super::{
        StalledStoreModule, assert_the_store_was_waited_out, one_per_minute, within_twice_the_bound,
    };

    /// The compile witness for `WsGuard`.
    #[gateway(path = "/ws/rated")]
    struct RatedGateway;

    #[messages]
    impl RatedGateway {
        #[subscribe_message("tick")]
        #[use_guards(ThrottlerGuard)]
        #[public]
        async fn tick(&self) -> &'static str {
            "ok"
        }
    }

    #[module(
        imports = [WsModule, ThrottlerModule::for_root(one_per_minute())],
        providers = [RatedGateway],
    )]
    struct RatedWsModule;

    #[module(
        imports = [
            WsModule,
            ThrottlerModule::for_root(one_per_minute()),
            StalledStoreModule,
        ],
        providers = [RatedGateway],
    )]
    struct StalledWsModule;

    /// The boot proves the wiring, and the entry is then exercised on the guard
    /// the container built.
    #[tokio::test]
    async fn a_second_message_on_one_connection_is_refused() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<RatedWsModule>()
            .await
            .expect("a #[subscribe_message] binding ThrottlerGuard boots");
        let guard = app
            .container()
            .get::<ThrottlerGuard>()
            .expect("ThrottlerModule registers the guard as global infrastructure");

        let client = WsClient::for_test();
        let data = serde_json::Value::Null;

        guard
            .check_ws_message(&client, "tick", &data)
            .await
            .expect("the first message on this connection is inside the budget");

        guard
            .check_ws_message(&client, "tick", &data)
            .await
            .expect_err("the second message inside the window is refused");

        guard
            .check_ws_message(&client, "other", &data)
            .await
            .expect("a different event on the same connection has its own bucket");

        let event = logs.expect_one(nest_rs_throttler::TARGET, "rate limit exceeded");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("transport").as_deref(), Some("ws"));
        assert_eq!(
            event.field("event").as_deref(),
            Some("tick"),
            "the event names the unit, got {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("connection").as_deref(),
            Some(client.id().to_string()).as_deref(),
            "…and the connection the bucket is keyed on, a number this process \
             minted for the socket — never the peer's address, got {:?}",
            event.fields,
        );
        assert_eq!(event.field("client"), None, "got {:?}", event.fields);
        assert!(event.field("retry_after").is_some());
    }

    /// No request timeout reaches a socket's messages at all.
    #[tokio::test(start_paused = true)]
    async fn a_store_that_never_answers_refuses_the_message_at_the_bound() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<StalledWsModule>()
            .await
            .expect("a #[subscribe_message] over a custom store boots");
        let guard = app
            .container()
            .get::<ThrottlerGuard>()
            .expect("ThrottlerModule registers the guard as global infrastructure");

        let sent = tokio::time::Instant::now();
        let refused = within_twice_the_bound(guard.check_ws_message(
            &WsClient::for_test(),
            "tick",
            &serde_json::Value::Null,
        ))
        .await
        .expect_err("a store that cannot answer refuses the message");

        assert!(sent.elapsed() >= nest_rs_throttler::HIT_TIMEOUT);
        assert_eq!(
            refused.http_status(),
            429,
            "refused as a rate limit, the one refusal a store that cannot answer gives",
        );
        assert_the_store_was_waited_out(&logs, "ws");
    }
}
