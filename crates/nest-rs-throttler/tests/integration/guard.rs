//! [`ThrottlerGuard`] bound **globally** against a route's `#[meta(Throttle)]`
//! (`src/guard.rs`) — the scope half of the contract, as opposed to the
//! per-route wiring `wiring.rs` covers.
//!
//! The class this closes: three doc comments and two documentation pages said a
//! global guard "runs before routing has resolved a handler, so no route
//! metadata is attached at that point" — from which a pooled `ThrottlerGuard`
//! would silently fall back to the module default and a route's own
//! `#[meta(Throttle)]` would mean nothing. Only a status code settles it, and
//! nothing asserted one: the pool executes at the `RouteShaper`, which
//! `#[routes]` wraps *inside* the `#[meta]` / `#[public]` route-data wrap, so
//! every guard on the chain reads the same metadata whichever scope declared
//! it.
//!
//! The module default is pinned generously (60/minute) on purpose: it is the
//! only other limit in the app, so a `429` on the third request cannot come
//! from anywhere but the route's own declaration — and the unmetered control
//! route proves the pinned default really is that generous rather than the
//! deployment's.
//!
//! The guard's other half is its bound on the store, `HIT_TIMEOUT`: every edge
//! the guard serves is driven below over a store that never answers, on paused
//! time, so the twenty seconds it waits cost the suite nothing.

use std::any::TypeId;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{ContainerBuilder, Module, module};
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

// No `#[use_guards]` at either scope — the pool is the only thing that can
// reach these routes, which is the whole point of the fixture.
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
}

#[module(
    imports = [
        ThrottlerModule::for_root(ThrottlerConfig {
            limit: Some(60),
            window_secs: Some(60),
        }),
    ],
    providers = [RatedController],
)]
struct RatedModule;

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

    // The third request exceeds the route's `#[meta(Throttle::per_minute(2))]`
    // and nothing else — the pinned module default is 60/minute. A `200` here
    // would mean the pool ran with no route metadata attached.
    let denied = app.http().get("/rated/strict").send().await;
    denied.assert_status(StatusCode::TOO_MANY_REQUESTS);
    denied.assert_header_exist("retry-after");

    // The `429` tells the client it was throttled; only the event says *whose*
    // bucket filled. A rate-limit denial ranks with the security events an
    // incident queries, so the route and the caller are two fields —
    // never the composite store key, which an operator would have to split on
    // U+001F to filter by either half.
    let event = logs.expect_one(nest_rs_throttler::TARGET, "rate limit exceeded");
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("route")
            .is_some_and(|r| r.contains("/rated/strict")),
        "the event names the route on its own field, got {:?}",
        event.fields,
    );
    assert!(
        event.field("client").is_some(),
        "the event names the caller on its own field, got {:?}",
        event.fields,
    );
    assert!(
        event.field("retry_after").is_some(),
        "the event carries the wait it told the client about, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_route_with_no_metadata_falls_back_to_the_module_default() {
    let app = app().await;

    // Same pooled guard, same three requests, no `#[meta]` on the route: the
    // pinned 60/minute applies, so nothing is refused. This is what makes the
    // sibling test's `429` attributable to the metadata rather than to a low
    // default the deployment (or a stray `NESTRS_THROTTLER__LIMIT`) supplied.
    for _ in 0..3 {
        app.http()
            .get("/rated/lenient")
            .send()
            .await
            .assert_status_is_ok();
    }
}

// --- a store that never answers ---------------------------------------------
//
// A store that cannot answer denies, and a store that never answers could not
// even do that: it held the unit of work for as long, and on the three edges no
// request timeout covers — a GraphQL subscription, an MCP operation, a socket's
// messages — for good. The guard waits `HIT_TIMEOUT` and no longer, then refuses
// the caller as a store that cannot answer does, on every edge it serves.

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

/// A store that answers, and slowly: just inside the guard's bound, as a store
/// running out its own budget does. It stands for the Redis store at the end of
/// a reconnect, whose connection budget sits well inside the bound.
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
/// port's remedy, so it supersedes the in-process default wherever it sits in
/// `imports` — the documented wiring of a custom store.
struct StoreModule<S>(std::marker::PhantomData<S>);

impl<S: ThrottlerStore + Default> Module for StoreModule<S> {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        builder
            .provide_declared_factory::<Arc<dyn ThrottlerStore>, _, _>(BACKEND_REMEDY, |_| async {
                Ok(Arc::new(S::default()) as Arc<dyn ThrottlerStore>)
            })
    }
}

type StalledStoreModule = StoreModule<StalledStore>;
type SlowStoreModule = StoreModule<SlowStore>;

/// The refusal alone reads like any other: a `429`, a `Retry-After`. What says
/// the store never answered — and which store it was — is this line, naming the
/// edge, the store and how long it was waited on.
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
/// guard that stopped bounding its store fails here, naming the bound, rather
/// than holding the suite for good the way it held the unit of work.
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
        }),
        SlowStoreModule,
    ],
    providers = [StalledController],
)]
struct SlowHttpModule;

/// Fail closed at the bound: the request a stalled store held for good is
/// refused once `HIT_TIMEOUT` passes — as a store that cannot answer refuses it,
/// a `429` for the whole window — and before the HTTP edge's own request
/// timeout, so the caller reads the guard's refusal and the operator the guard's
/// line naming the store, rather than a `503` naming nothing.
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
    // bound would then be past it, and a hung store would read one way on HTTP
    // and another on every edge no request timeout covers.
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

/// The other side of the bound: a store that answers inside it is waited for,
/// however slowly, and its answer is the one the caller gets. The bound is a
/// net under a store that never answers, never a budget cutting short one that
/// does — the Redis store's own budget answers well inside it.
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

/// The two budgets either side of the bound, read from the constants that set
/// them rather than retyped: the HTTP edge's request timeout must stay above
/// it, or a stalled store reads as a `503` on HTTP and as the guard's refusal
/// everywhere else.
#[test]
fn the_bound_answers_before_the_http_edge_times_a_request_out() {
    let timeout = HttpConfig::default()
        .request_timeout
        .expect("the HTTP edge bounds a request by default");
    assert!(HIT_TIMEOUT < timeout, "{HIT_TIMEOUT:?} vs {timeout:?}");
}

// --- the three edges an HTTP-only guard left unmetered -----------------------
//
// `/graphql` and `/mcp` are `EdgePosture::Exempt` and a WS message runs after
// the upgrade's chain has returned, so none of the three is reachable from an
// HTTP-scope binding at any scope. A `ThrottlerGuard` that implemented only
// `check_http` therefore rate-limited nothing there — including the
// anonymous-reachable `/graphql` mount — while compiling, reading as a
// protection, and (before the capability markers) not even refusing the
// binding.
//
// Each module below binds the guard where a developer would, which is also the
// only witness its capability marker has: `#[operations]`, `#[tools]` and
// `#[messages]` emit a bound against `GraphqlGuard` / `McpGuard` / `WsGuard`, so
// an unattested guard is a compile error at the `#[use_guards]` line.

/// One request per minute, so the *second* call is the assertion and the first
/// proves the site was reachable at all.
#[cfg(any(feature = "graphql", feature = "mcp", feature = "ws"))]
fn one_per_minute() -> ThrottlerConfig {
    ThrottlerConfig {
        limit: Some(1),
        window_secs: Some(60),
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

        // The gate is the whole point: `/graphql` is `Exempt`, so before this
        // entry existed the operation ran however many times a client asked.
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

        // Said once per process, with the remedy, and **only for an anonymous
        // caller** — which this fixture is. An authenticated one keys on its
        // actor and shares nothing; keying every caller together was a limiter
        // one client could turn into a denial of service for the rest.
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

    /// A field is refused at the bound when its store never answers, and its
    /// resolver never runs. Here a query's, which the HTTP edge would otherwise
    /// have answered with a `503` at its own timeout; a subscription's fields,
    /// carried by a socket, have no such timeout at all.
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

    // `AllowAllMcpGuard` is the endpoint's deliberate opt-out: without an
    // operation guard `/mcp` is deny-all, and a `401` would hide whether the
    // per-operation chain ran at all.
    #[module(
        imports = [ThrottlerModule::for_root(one_per_minute())],
        providers = [RatedTool, AllowAllMcpGuard as dyn McpOperationGuard],
    )]
    struct RatedMcpModule;

    // The stream's keep-alive is off: `sse-stream` re-arms it from the real
    // clock, which a paused one leaves behind at every jump, so it fired over and
    // over — five real seconds for these twenty paused ones — and pings are not
    // what this test is about.
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

    /// A tool call is refused at the bound when its store never answers. The
    /// call answers over a stream, which runs after the handler the HTTP edge's
    /// request timeout covers has returned, so the guard's bound is the only one
    /// the operation has — the edge is mounted here, and does not end it.
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

    /// The binding a developer writes, and the compile witness for `WsGuard`:
    /// `#[messages]` bounds every per-message guard on it, so this file would
    /// not build if `ThrottlerGuard` stopped attesting the edge.
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

    /// The message chain is frozen at mount, behind a real socket the harness
    /// has no driver for — `TestApp` drives HTTP, GraphQL and MCP, and a WS
    /// gateway only through its upgrade. So the boot proves the wiring (the
    /// access graph resolves the guard, the mount succeeds) and the entry is
    /// then exercised on the guard the container actually built, rather than on
    /// one this test hand-assembled over a store nothing configured.
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

        // A WS message is dispatched after the upgrade returned, so nothing an
        // HTTP-scope binding does can meter this.
        guard
            .check_ws_message(&client, "tick", &data)
            .await
            .expect_err("the second message inside the window is refused");

        // The event is half the bucket: a flood of `tick` must not spend the
        // budget of every other message the gateway serves.
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
            event.field("client").as_deref(),
            Some(client.id().to_string()).as_deref(),
            "…and the connection as the caller half — the peer address that \
             keyed the upgrade is gone by the time a message runs, got {:?}",
            event.fields,
        );
        assert!(event.field("retry_after").is_some());
    }

    /// A message is refused at the bound when its store never answers. No
    /// request timeout reaches a socket's messages at all, so before the bound a
    /// stalled store held this connection's traffic for good.
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
