//! Covers `src/guard.rs`.
//!
//! `AUTHENTICATE_TIMEOUT` is driven over a strategy that never answers, on the
//! guard alone and on every edge it authenticates for, on paused time.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
use nest_rs_core::{injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_testing::{LogCapture, TestApp};
use poem::Request;
use poem::http::StatusCode;

struct AuthenticateAs(&'static str);

#[async_trait]
impl Strategy for AuthenticateAs {
    type Principal = &'static str;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Ok(self.0)
    }
}

struct FailWith;

#[async_trait]
impl Strategy for FailWith {
    type Principal = ();

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

struct RejectWith(fn() -> AuthError);

#[async_trait]
impl Strategy for RejectWith {
    type Principal = ();

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err((self.0)())
    }
}

/// A request carrying the `#[public]` marker the route macro attaches.
fn public_request() -> Request {
    let mut req = crate::request(&[]);
    req.extensions_mut().insert(nest_rs_http::Public);
    req
}

#[tokio::test]
async fn attaches_principal_on_success() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateAs("alice")));
    let mut req = crate::request(&[]);

    guard.check_http(&mut req).await.expect("guard passes");
    assert_eq!(req.extensions().get::<&'static str>(), Some(&"alice"));
}

#[tokio::test]
async fn strategy_error_denies_as_unauthorized() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    let mut req = crate::request(&[]);

    let denial = guard.check_http(&mut req).await.expect_err("auth failed");
    assert!(matches!(denial, Denial::Unauthorized { .. }));
    assert!(req.extensions().get::<&'static str>().is_none());
}

#[tokio::test]
async fn public_route_admits_an_anonymous_caller() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    guard
        .check_http(&mut public_request())
        .await
        .expect("no credential on a public route is not a failure");
}

#[tokio::test]
async fn public_route_admits_a_rejected_credential_as_anonymous() {
    // The posture `#[public]` promises: a forged token does not turn a public
    // route into a 401 — it is logged and the request continues anonymously.
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| AuthError::InvalidSignature)));
    guard
        .check_http(&mut public_request())
        .await
        .expect("a rejected credential still leaves the public route reachable");

    // The request succeeds, so this `warn` is the only trace a forged token leaves.
    let event = logs.expect_one(
        nest_rs_authn::TARGET,
        "rejected credential on a public route — continuing as anonymous",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("reason").as_deref(), Some("invalid_signature"));
    assert!(
        event.field("strategy").is_some(),
        "the event names the strategy that rejected it, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn unreachable_store_fails_closed_even_on_a_public_route() {
    // The credential was never evaluated, so serving the caller as anonymous
    // would silently downgrade every authenticated session during an outage.
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| AuthError::Unavailable {
        detail: "store unreachable".into(),
        retry_after: None,
    })));

    let denial = guard
        .check_http(&mut public_request())
        .await
        .expect_err("an unevaluated credential must not pass as anonymous");
    assert!(
        matches!(
            denial,
            Denial::Unavailable {
                retry_after_secs: None,
                ..
            }
        ),
        "{denial:?}"
    );
    assert_eq!(denial.http_status(), 503, "the caller did nothing wrong");

    // The client message is opaque, so the outage is only readable here.
    let event = logs.expect_one(
        nest_rs_authn::TARGET,
        "authentication unavailable — what the strategy asks did not answer",
    );
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("store unreachable")),
        "the event carries the underlying failure, got {:?}",
        event.fields,
    );
    assert!(event.field("strategy").is_some(), "{:?}", event.fields);
}

/// Authentication sets the ambient identity everything downstream reads back
/// through `current_actor_id()`, without the handler threading it down.
#[tokio::test]
async fn a_successful_check_publishes_the_actor_into_the_ambient_context() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateAs("ada")));
    let correlation = nest_rs_core::Correlation::minted(None);

    let seen = nest_rs_core::with_request_scope(None, correlation, async {
        let mut req = Request::default();
        guard
            .check_http(&mut req)
            .await
            .expect("the strategy authenticates");
        nest_rs_core::current_actor_id()
    })
    .await;

    assert_eq!(
        seen.as_deref(),
        Some("ada"),
        "the actor must be readable below the guard without being threaded through",
    );
}

/// An anonymous caller is reported as **absent**, never as a sentinel string: a
/// query counting anonymous traffic must not also count an actor genuinely named
/// `""` or `"anonymous"`.
#[tokio::test]
async fn an_unauthenticated_caller_has_no_ambient_actor() {
    let correlation = nest_rs_core::Correlation::minted(None);

    let seen = nest_rs_core::with_request_scope(None, correlation, async {
        // No guard ran at all — the shape of every request before authentication
        // and of every `#[public]` route reached without a credential.
        nest_rs_core::current_actor_id()
    })
    .await;

    assert_eq!(seen, None);
}

// The bound: a strategy that never answers fails closed on every route,
// `#[public]` included, as an unreachable identity store does.

/// A strategy that never answers — a backend holding the call, a network
/// dropping it without a reset.
#[injectable]
#[derive(Default)]
struct StalledStrategy;

#[async_trait]
impl Strategy for StalledStrategy {
    type Principal = &'static str;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        std::future::pending().await
    }
}

/// A strategy that answers, and slowly: just inside the guard's bound, as a
/// social login does whose provider calls each ran out most of their own budget.
struct SlowStrategy;

#[async_trait]
impl Strategy for SlowStrategy {
    type Principal = &'static str;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        tokio::time::sleep(AUTHENTICATE_TIMEOUT - Duration::from_millis(1)).await;
        Ok("ada")
    }
}

/// The guard over the strategy that never answers, as an app aliases its own.
type StalledAuthn = AuthnGuard<StalledStrategy>;

/// The line a strategy that did not answer is filed under — the one thing that
/// tells this denial from an unreachable store's, which answers the caller the
/// same way.
const STALLED: &str = "strategy did not answer within the guard's timeout; denying (fail-closed)";

/// What every handler below answers, so a response carrying it is a request the
/// guard let through.
const REACHED: &str = "reached";

/// The refusal alone reads like an outage; this line says the strategy never
/// answered, which strategy, on which edge, and how long it was waited on.
fn assert_the_strategy_was_waited_out(logs: &LogCapture) {
    let cause = logs.expect_one(nest_rs_authn::TARGET, STALLED);
    assert_eq!(cause.level, "warn");
    assert_eq!(
        cause.field("strategy").as_deref(),
        Some(std::any::type_name::<StalledStrategy>()),
        "the line names the strategy, got {:?}",
        cause.fields,
    );
    assert_eq!(cause.field("transport").as_deref(), Some("http"));
    assert_eq!(
        cause.field("waited_ms").as_deref(),
        Some(AUTHENTICATE_TIMEOUT.as_millis().to_string().as_str()),
    );
}

/// Await `unit` on the paused clock for twice the bound and no longer, so a
/// guard that stopped bounding its strategy fails here, naming the bound,
/// rather than holding the suite for good the way it held the request.
async fn within_twice_the_bound<T>(unit: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(AUTHENTICATE_TIMEOUT * 2, unit)
        .await
        .unwrap_or_else(|_| {
            panic!("no answer within twice AUTHENTICATE_TIMEOUT ({AUTHENTICATE_TIMEOUT:?})")
        })
}

#[tokio::test(start_paused = true)]
async fn a_strategy_that_never_answers_is_denied_at_the_bound() {
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(StalledStrategy));
    let mut req = crate::request(&[]);

    let sent = tokio::time::Instant::now();
    let denial = within_twice_the_bound(guard.check_http(&mut req))
        .await
        .expect_err("a strategy that never answered decided nothing, so nothing passes");

    assert!(
        sent.elapsed() >= AUTHENTICATE_TIMEOUT,
        "denied at the bound, not before: {:?}",
        sent.elapsed(),
    );
    // The answer an unreachable identity store gets, word for word: the caller
    // cannot tell a hang from an outage, and has the same remedy for both.
    let unavailable = AuthError::Unavailable {
        detail: String::new(),
        retry_after: None,
    };
    assert!(matches!(denial, Denial::Unavailable { .. }), "{denial:?}");
    assert_eq!(denial.message(), unavailable.client_message());
    assert!(
        req.extensions().get::<&'static str>().is_none(),
        "no principal is attached",
    );
    assert_the_strategy_was_waited_out(&logs);
}

/// `#[public]` absorbs a *rejected* credential, never one the strategy never
/// looked at.
#[tokio::test(start_paused = true)]
async fn a_strategy_that_never_answers_is_denied_on_a_public_route_too() {
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(StalledStrategy));

    let denial = within_twice_the_bound(guard.check_http(&mut public_request()))
        .await
        .expect_err("an unevaluated credential must not pass as anonymous");

    assert!(matches!(denial, Denial::Unavailable { .. }), "{denial:?}");
    assert_the_strategy_was_waited_out(&logs);
    logs.expect_none(nest_rs_authn::TARGET, "anonymous request on a public route");
}

/// A strategy that answers inside the bound is waited for, however slowly: the
/// bound is a net, not a budget.
#[tokio::test(start_paused = true)]
async fn a_strategy_answering_inside_the_bound_is_waited_for() {
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(SlowStrategy));
    let mut req = crate::request(&[]);

    within_twice_the_bound(guard.check_http(&mut req))
        .await
        .expect("a strategy that answers in time is heard");

    assert_eq!(req.extensions().get::<&'static str>(), Some(&"ada"));
    logs.expect_none(nest_rs_authn::TARGET, STALLED);
}

/// The budget above the bound, read from the constant that sets it rather than
/// retyped: the HTTP edge's request timeout must stay above it, or a hung
/// strategy reads as the edge's `503`, naming nothing, instead of the guard's
/// denial and the guard's line naming the strategy.
#[test]
fn the_bound_answers_before_the_http_edge_times_a_request_out() {
    let timeout = HttpConfig::default()
        .request_timeout
        .expect("the HTTP edge bounds a request by default");
    assert!(
        AUTHENTICATE_TIMEOUT < timeout,
        "{AUTHENTICATE_TIMEOUT:?} vs {timeout:?}"
    );
}

// Every edge reaches the guard's one entry, `check_http`. Each app mounts the
// HTTP edge at its default request timeout, so a refusal is shown to come from
// the guard's bound first.

/// The HTTP edge the apps below mount — the budget the guard's bound answers
/// inside.
fn edge() -> HttpConfig {
    HttpConfig {
        port: 0,
        ..Default::default()
    }
}

/// A refusal at the bound and not at the edge's timeout: at least the bound
/// waited, and less than the edge's request timeout.
fn assert_refused_at_the_bound(waited: Duration) {
    assert!(
        waited >= AUTHENTICATE_TIMEOUT,
        "refused at the bound, not before: {waited:?}"
    );
    let timeout = edge()
        .request_timeout
        .expect("the HTTP edge bounds a request by default");
    assert!(
        waited < timeout,
        "…and before the edge's own request timeout: {waited:?}"
    );
}

#[controller(path = "/stalled")]
#[use_guards(StalledAuthn)]
struct StalledController;

#[routes]
impl StalledController {
    #[get("/private")]
    async fn private(&self) -> &'static str {
        REACHED
    }

    #[get("/public")]
    #[public]
    async fn public(&self) -> &'static str {
        REACHED
    }
}

#[module(
    imports = [HttpModule::for_root(edge())],
    providers = [StalledStrategy, StalledAuthn, StalledController],
)]
struct StalledHttpModule;

/// A route is refused at the bound, a `#[public]` one as much as a guarded one,
/// with the `503` an unevaluated credential gets — the caller did nothing wrong
/// — and before the edge's own request timeout would have answered.
#[tokio::test(start_paused = true)]
async fn a_route_is_refused_at_the_bound() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<StalledHttpModule>()
        .await
        .expect("a controller binding the guard boots");

    for path in ["/stalled/private", "/stalled/public"] {
        let sent = tokio::time::Instant::now();
        let refused = within_twice_the_bound(app.http().get(path).send()).await;
        assert_refused_at_the_bound(sent.elapsed());
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        let body = refused.0.into_body().into_string().await.expect("a body");
        assert!(
            !body.contains(REACHED),
            "{path}: the handler never ran: {body}"
        );
    }
    assert_eq!(
        logs.find(nest_rs_authn::TARGET, STALLED).len(),
        2,
        "one line per request the strategy held",
    );
}

mod graphql {
    use nest_rs_core::module;
    use nest_rs_graphql::async_graphql::Result as GraphqlResult;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_guards::guard;
    use nest_rs_http::HttpModule;
    use nest_rs_testing::{LogCapture, TestApp};
    use poem::http::StatusCode;

    use super::{
        REACHED, StalledAuthn, StalledStrategy, assert_refused_at_the_bound,
        assert_the_strategy_was_waited_out, edge, within_twice_the_bound,
    };

    #[resolver]
    struct TickResolver;

    #[operations]
    impl TickResolver {
        #[query]
        #[public]
        async fn tick(&self) -> GraphqlResult<String> {
            Ok(REACHED.to_owned())
        }
    }

    #[module(
        imports = [HttpModule::for_root(edge()), GraphqlModule::for_root(None)],
        providers = [StalledStrategy, StalledAuthn, TickResolver],
    )]
    struct StalledGraphqlModule;

    /// The GraphQL POST is authenticated by the global pool's fallback, which
    /// marks the request `#[public]` so an anonymous caller reaches the
    /// resolvers' own gates. A strategy that never answered is not that caller,
    /// and the POST is refused at the bound rather than served anonymously.
    #[tokio::test(start_paused = true)]
    async fn the_graphql_post_is_refused_at_the_bound() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<StalledGraphqlModule>()
            .use_guards_global([guard::<StalledAuthn>()])
            .build()
            .await
            .expect("a resolver under a pooled AuthnGuard boots");

        let sent = tokio::time::Instant::now();
        let refused = within_twice_the_bound(
            app.http()
                .post("/graphql")
                .body_json(&serde_json::json!({ "query": "{ tick }" }))
                .send(),
        )
        .await;
        assert_refused_at_the_bound(sent.elapsed());
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        let body = refused.0.into_body().into_string().await.expect("a body");
        assert!(!body.contains(REACHED), "the resolver never ran: {body}");
        assert_the_strategy_was_waited_out(&logs);
    }
}

mod mcp {
    use nest_rs_core::module;
    use nest_rs_guards::guard;
    use nest_rs_http::HttpModule;
    use nest_rs_mcp::{McpError, mcp, tools};
    use nest_rs_testing::mcp::initialize_request;
    use nest_rs_testing::{LogCapture, TestApp};
    use poem::http::StatusCode;

    use super::{
        REACHED, StalledAuthn, StalledStrategy, assert_refused_at_the_bound,
        assert_the_strategy_was_waited_out, edge, within_twice_the_bound,
    };

    #[mcp]
    #[derive(Clone, Default)]
    struct TickTool;

    #[tools]
    impl TickTool {
        /// Answer with a constant — the assertions are about what ran around it.
        #[tool]
        #[public]
        async fn tick(&self) -> Result<String, McpError> {
            Ok(REACHED.to_owned())
        }
    }

    #[module(
        imports = [HttpModule::for_root(edge())],
        providers = [StalledStrategy, StalledAuthn, TickTool],
    )]
    struct StalledMcpModule;

    /// The MCP POST is authenticated in the endpoint's `before`, by the global
    /// pool's fallback, ahead of any operation — so the bound refuses the
    /// request before rmcp has opened a stream.
    #[tokio::test(start_paused = true)]
    async fn the_mcp_post_is_refused_at_the_bound() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<StalledMcpModule>()
            .use_guards_global([guard::<StalledAuthn>()])
            .build()
            .await
            .expect("an #[mcp] host under a pooled AuthnGuard boots");

        let sent = tokio::time::Instant::now();
        let refused = within_twice_the_bound(
            app.http()
                .post(nest_rs_mcp::DEFAULT_PATH)
                .body_json(&initialize_request())
                .send(),
        )
        .await;
        assert_refused_at_the_bound(sent.elapsed());
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        assert_the_strategy_was_waited_out(&logs);
    }
}

mod ws {
    use nest_rs_core::module;
    use nest_rs_http::HttpModule;
    use nest_rs_testing::{LogCapture, TestApp};
    use nest_rs_ws::{WsModule, gateway, messages};
    use poem::http::{StatusCode, header};

    use super::{
        REACHED, StalledAuthn, StalledStrategy, assert_refused_at_the_bound,
        assert_the_strategy_was_waited_out, edge, within_twice_the_bound,
    };

    const PATH: &str = "/ws/stalled";

    #[gateway(path = "/ws/stalled")]
    #[use_guards(StalledAuthn)]
    struct StalledGateway;

    #[messages]
    impl StalledGateway {
        #[subscribe_message("tick")]
        #[public]
        async fn tick(&self) -> &'static str {
            REACHED
        }
    }

    #[module(
        imports = [HttpModule::for_root(edge()), WsModule],
        providers = [StalledStrategy, StalledAuthn, StalledGateway],
    )]
    struct StalledWsModule;

    /// A gateway's guards run on its upgrade, an HTTP `GET`, so the bound
    /// refuses the upgrade itself and no socket opens. An in-process client
    /// cannot finish a handshake that *passed* either — it answers `500 no
    /// upgrade` at once — so what shows the guard refused it is the wait and
    /// the two lines: the guard's, and the chain's naming the guard.
    #[tokio::test(start_paused = true)]
    async fn the_websocket_upgrade_is_refused_at_the_bound() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<StalledWsModule>()
            .await
            .expect("a gateway binding the guard boots");

        let sent = tokio::time::Instant::now();
        let refused = within_twice_the_bound(
            app.http()
                .get(PATH)
                .header(header::UPGRADE, "websocket")
                .header(header::CONNECTION, "upgrade")
                .header(header::SEC_WEBSOCKET_VERSION, "13")
                .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
                .send(),
        )
        .await;
        assert_refused_at_the_bound(sent.elapsed());
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        assert_the_strategy_was_waited_out(&logs);

        let denied = logs.expect_one(nest_rs_core::target::LAYERS, "guard denied the request");
        assert_eq!(denied.field("status").as_deref(), Some("503"));
        assert!(
            denied
                .field("guard")
                .is_some_and(|guard| guard.contains("StalledStrategy")),
            "the chain names the guard that refused the upgrade, got {:?}",
            denied.fields,
        );
    }
}

/// The guard declares its net around its strategy, so the boot holds every
/// budget the strategy's code can reach under `AUTHENTICATE_TIMEOUT` — the
/// guard would otherwise deny a request the store was still answering, its
/// cause lost — and leaves alone what that code cannot reach.
mod nets {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
    use nest_rs_core::{App, Budget, BudgetPastNetError, injectable, module};
    use poem::Request;

    /// A store a strategy resolves identities in, waiting what it holds for an
    /// answer.
    struct IdentityStore(Duration);

    fn store_budget() -> Budget {
        Budget::of::<IdentityStore>("the identity store", "IDENTITY_STORE_WAIT", |store| {
            Some(store.0)
        })
    }

    #[injectable]
    struct StoreBackedStrategy {
        #[inject]
        #[expect(
            dead_code,
            reason = "injected only so the strategy's code reaches the store"
        )]
        store: Arc<IdentityStore>,
    }

    #[async_trait]
    impl Strategy for StoreBackedStrategy {
        type Principal = ();

        async fn authenticate(&self, _req: &mut Request) -> Result<(), AuthError> {
            Err(AuthError::MissingCredentials)
        }
    }

    type StoreBackedGuard = AuthnGuard<StoreBackedStrategy>;

    #[module(providers = [StoreBackedStrategy, StoreBackedGuard])]
    struct StoreBackedModule;

    #[injectable]
    struct LocalStrategy;

    #[async_trait]
    impl Strategy for LocalStrategy {
        type Principal = ();

        async fn authenticate(&self, _req: &mut Request) -> Result<(), AuthError> {
            Err(AuthError::MissingCredentials)
        }
    }

    type LocalGuard = AuthnGuard<LocalStrategy>;

    #[module(providers = [LocalStrategy, LocalGuard])]
    struct LocalModule;

    #[tokio::test]
    async fn a_budget_the_strategy_reaches_at_the_guards_net_fails_the_boot() {
        let Err(refused) = App::builder()
            .provide(IdentityStore(AUTHENTICATE_TIMEOUT))
            .provide_meta(store_budget())
            .module::<StoreBackedModule>()
            .build()
            .await
        else {
            panic!("a store the strategy waits on at the guard's net must not boot");
        };
        let refused = refused
            .downcast::<BudgetPastNetError>()
            .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
        assert_eq!(
            (refused.resource, refused.port, refused.net),
            (
                "the identity store",
                "the authentication guard",
                AUTHENTICATE_TIMEOUT
            )
        );
    }

    #[tokio::test]
    async fn a_budget_the_strategy_cannot_reach_is_left_alone() {
        App::builder()
            .provide(IdentityStore(AUTHENTICATE_TIMEOUT * 2))
            .provide_meta(store_budget())
            .module::<LocalModule>()
            .build()
            .await
            .expect("a strategy reaching no store boots beside a slow one");
    }
}
