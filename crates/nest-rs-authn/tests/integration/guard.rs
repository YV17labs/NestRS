//! Covers `src/guard.rs`.
//!
//! The guard runs through the HTTP edge's guard stage, as the unit a route of
//! each posture declares. `AUTHENTICATE_TIMEOUT` is driven over a strategy that
//! never answers, on the guard alone and on every edge it authenticates for, on
//! paused time.

use std::any::TypeId;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, PrincipalIdentity, Strategy};
use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
use nest_rs_core::{AmbientValue, Handler, Posture, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::{attached, check_request};
use nest_rs_http::{HttpConfig, HttpModule, RejectedCredential, controller, routes};
use nest_rs_testing::{LogCapture, TestApp};
use poem::Request;
use poem::http::StatusCode;

/// A principal with an audit identity.
#[derive(Clone, Debug, PartialEq)]
struct Who(&'static str);

impl AmbientValue for Who {}

impl PrincipalIdentity for Who {
    fn actor_id(&self) -> Option<String> {
        Some(self.0.to_owned())
    }
}

/// A principal with none, as a machine client's is.
#[derive(Clone, Debug)]
struct Nobody;

impl AmbientValue for Nobody {}

impl PrincipalIdentity for Nobody {
    fn actor_id(&self) -> Option<String> {
        None
    }
}

struct Routes;

/// Declare a route of `posture`, as `#[routes]` declares each handler.
const fn route(method: &'static str, posture: Posture) -> Handler {
    declare_handler(HandlerDeclaration {
        unit: nest_rs_http::unit::REQUEST,
        host: TypeId::of::<Routes>(),
        host_name: "Routes",
        method,
        address: "/",
        posture,
        meta: MetaLevels::default,
    })
}

/// A route with no posture of its own, gated by its guards alone.
static GATED: Handler = route("gated", Posture::Gated);

/// A `#[public]` route.
static PUBLIC: Handler = route("public", Posture::Public);

/// An in-band endpoint, as `/graphql` declares itself: its operations each
/// declare their own posture.
static IN_BAND: Handler = route("in_band", Posture::InBand);

/// Run `guard` on `req` as the HTTP edge runs it on the route `site`.
async fn check(
    guard: &dyn Guard,
    site: &'static Handler,
    req: &mut nest_rs_http::Request,
) -> Result<(), Denial> {
    check_request(site, [("AuthnGuard", guard)], req).await
}

/// A request with no credential, as the edge hands it to its guards.
fn bare() -> nest_rs_http::Request {
    nest_rs_http::Request::builder().finish()
}

struct AuthenticateAs(&'static str);

#[async_trait]
impl Strategy for AuthenticateAs {
    type Principal = Who;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Ok(Who(self.0))
    }
}

struct FailWith;

#[async_trait]
impl Strategy for FailWith {
    type Principal = Nobody;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

struct RejectWith(fn() -> AuthError);

#[async_trait]
impl Strategy for RejectWith {
    type Principal = Nobody;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err((self.0)())
    }
}

#[tokio::test]
async fn attaches_principal_on_success() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateAs("alice")));
    let mut req = bare();

    check(&guard, &GATED, &mut req).await.expect("guard passes");
    assert_eq!(attached::<Who>(&req).as_deref(), Some(&Who("alice")));
}

#[tokio::test]
async fn strategy_error_denies_as_unauthorized() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    let mut req = bare();

    let denial = check(&guard, &GATED, &mut req)
        .await
        .expect_err("auth failed");
    assert!(matches!(denial, Denial::Unauthorized { .. }));
    assert!(attached::<Nobody>(&req).is_none());
}

#[tokio::test]
async fn public_route_admits_an_anonymous_caller() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    check(&guard, &PUBLIC, &mut bare())
        .await
        .expect("no credential on a public route is not a failure");
}

#[tokio::test]
async fn public_route_admits_a_rejected_credential_as_anonymous() {
    // The posture `#[public]` promises: a forged token does not turn a public
    // route into a 401 — it is logged and the request continues anonymously.
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| AuthError::InvalidSignature)));
    let mut req = bare();
    check(&guard, &PUBLIC, &mut req)
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
    // …and the request carries the refusal a handler reading a principal
    // answers with, never an identity.
    let rejected = req
        .extensions()
        .get::<RejectedCredential>()
        .expect("the rejected credential is recorded on the request");
    assert_eq!(rejected.principal, TypeId::of::<Nobody>());
    assert!(attached::<Nobody>(&req).is_none());
}

#[tokio::test]
async fn an_in_band_request_admits_an_anonymous_caller() {
    let guard = AuthnGuard::new(Arc::new(FailWith));
    check(&guard, &IN_BAND, &mut bare())
        .await
        .expect("no credential on an in-band endpoint is left to its operations' gates");
}

/// No operation of an in-band request has declared itself public yet, so a
/// credential the caller presented is verified rather than set aside.
#[tokio::test]
async fn an_in_band_request_still_verifies_a_presented_credential() {
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| AuthError::InvalidSignature)));
    let denial = check(&guard, &IN_BAND, &mut bare())
        .await
        .expect_err("a rejected credential is refused on an in-band endpoint");
    assert_eq!(denial.http_status(), 401);
    assert_eq!(
        denial.bearer_error(),
        Some(nest_rs_http::challenge::INVALID_TOKEN)
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

    let denial = check(&guard, &PUBLIC, &mut bare())
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
        let mut req = bare();
        check(&guard, &GATED, &mut req)
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
    type Principal = Who;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        std::future::pending().await
    }
}

/// A strategy that answers, and slowly: just inside the guard's bound, as a
/// social login does whose provider calls each ran out most of their own budget.
struct SlowStrategy;

#[async_trait]
impl Strategy for SlowStrategy {
    type Principal = Who;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        tokio::time::sleep(AUTHENTICATE_TIMEOUT - Duration::from_millis(1)).await;
        Ok(Who("ada"))
    }
}

/// A strategy that unwinds — an `unwrap` on its store's answer, a poisoned
/// lock.
struct PanickingStrategy;

#[async_trait]
impl Strategy for PanickingStrategy {
    type Principal = Who;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        panic!("the strategy's store unwound")
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
    let mut req = bare();

    let sent = tokio::time::Instant::now();
    let denial = within_twice_the_bound(check(&guard, &GATED, &mut req))
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
    assert!(attached::<Who>(&req).is_none(), "no principal is attached");
    assert_the_strategy_was_waited_out(&logs);
}

/// A strategy that unwound decided nothing: the guard denies as it does past
/// its bound, on a `#[public]` route too, and files the panic once at `error`.
#[tokio::test]
async fn a_strategy_that_panics_is_denied_and_said_once() {
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(PanickingStrategy));

    for site in [&GATED, &PUBLIC] {
        let mut req = bare();
        let denial = check(&guard, site, &mut req)
            .await
            .expect_err("a strategy that unwound decided nothing, so nothing passes");
        assert!(matches!(denial, Denial::Unavailable { .. }), "{denial:?}");
        assert!(attached::<Who>(&req).is_none());
    }

    let said = logs.find(
        nest_rs_authn::TARGET,
        "strategy panicked; denying (fail-closed)",
    );
    assert_eq!(said.len(), 2, "one line per request: {:#?}", logs.events());
    assert_eq!(said[0].level, "error");
    assert_eq!(
        said[0].field("strategy").as_deref(),
        Some(std::any::type_name::<PanickingStrategy>()),
    );
    assert_eq!(said[0].field("transport").as_deref(), Some("http"));
    assert_eq!(
        said[0].field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the strategy's store unwound"),
    );
    logs.expect_none(nest_rs_authn::TARGET, "anonymous request on a public route");
}

/// `#[public]` absorbs a *rejected* credential, never one the strategy never
/// looked at.
#[tokio::test(start_paused = true)]
async fn a_strategy_that_never_answers_is_denied_on_a_public_route_too() {
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(StalledStrategy));

    let denial = within_twice_the_bound(check(&guard, &PUBLIC, &mut bare()))
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
    let mut req = bare();

    within_twice_the_bound(check(&guard, &GATED, &mut req))
        .await
        .expect("a strategy that answers in time is heard");

    assert_eq!(attached::<Who>(&req).as_deref(), Some(&Who("ada")));
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
    use async_trait::async_trait;
    use nest_rs_authn::{AuthError, AuthnGuard, Strategy};
    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::Result as GraphqlResult;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_guards::guard;
    use nest_rs_http::HttpModule;
    use nest_rs_testing::{LogCapture, TestApp};
    use poem::Request;
    use poem::http::{StatusCode, header};

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

    /// The GraphQL POST is authenticated by the global pool, in band: an
    /// anonymous caller reaches the resolvers' own gates. A strategy that never
    /// answered is not that caller, and the POST is refused at the bound rather
    /// than served anonymously.
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

    /// Authenticates `Bearer good`, refuses any other bearer, and reports a
    /// request carrying none as missing credentials.
    #[injectable]
    #[derive(Default)]
    struct BearerStrategy;

    #[async_trait]
    impl Strategy for BearerStrategy {
        type Principal = super::Who;

        async fn authenticate(&self, req: &mut Request) -> Result<Self::Principal, AuthError> {
            match req.headers().get(header::AUTHORIZATION) {
                None => Err(AuthError::MissingCredentials),
                Some(bearer) if bearer == "Bearer good" => Ok(super::Who("ada")),
                Some(_) => Err(AuthError::InvalidToken),
            }
        }
    }

    type BearerAuthn = AuthnGuard<BearerStrategy>;

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [BearerStrategy, BearerAuthn, TickResolver],
    )]
    struct BearerGraphqlModule;

    /// `/graphql` is in band: an anonymous caller reaches a `#[public]` query,
    /// while a bearer the caller presented is still verified, and a bad one is
    /// refused before any operation runs.
    #[tokio::test]
    async fn the_graphql_post_admits_an_anonymous_caller_and_refuses_a_bad_bearer() {
        let app = TestApp::builder()
            .module::<BearerGraphqlModule>()
            .use_guards_global([guard::<BearerAuthn>()])
            .build()
            .await
            .expect("a resolver under a pooled AuthnGuard boots");
        let tick = serde_json::json!({ "query": "{ tick }" });

        let anonymous = app.http().post("/graphql").body_json(&tick).send().await;
        anonymous.assert_status(StatusCode::OK);
        let body = anonymous.0.into_body().into_string().await.expect("a body");
        assert!(body.contains(REACHED), "the public query answered: {body}");

        let forged = app
            .http()
            .post("/graphql")
            .header(header::AUTHORIZATION, "Bearer forged")
            .body_json(&tick)
            .send()
            .await;
        forged.assert_status(StatusCode::UNAUTHORIZED);
        let challenge = forged
            .0
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(
            challenge.contains(r#"error="invalid_token""#),
            "the refusal names RFC 6750's code: {challenge}"
        );
        let body = forged.0.into_body().into_string().await.expect("a body");
        assert!(!body.contains(REACHED), "the resolver never ran: {body}");
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
    /// pool run in band, ahead of any operation — so the bound refuses the
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
    /// the guard's line, the one the denial files.
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

        // The guard filed the refusal, so the chain files none: one denial, one line.
        logs.expect_none(nest_rs_core::target::LAYERS, "guard denied the request");
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
        type Principal = super::Nobody;

        async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
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
        type Principal = super::Nobody;

        async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
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
