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
use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, install_pending};
use nest_rs_core::{Ambient, AmbientValue, Handler, Posture, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::{RejectedCredential, attached, attached_pending, check_request};
use nest_rs_http::{HttpConfig, HttpModule, RawBody, Request, StatusCode, controller, routes};
use nest_rs_testing::{LogCapture, TestApp};

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
    assert_eq!(denial.http_status(), 401);
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
    assert_eq!(
        event.field("strategy").as_deref(),
        Some("RejectWith"),
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

/// What a route's document lists for the guard is what it answers: every
/// refusal on a route of either posture carries a status declared for it.
#[tokio::test]
async fn every_refusal_carries_a_status_the_guard_declares_for_the_route() {
    let outcomes: [fn() -> AuthError; 5] = [
        || AuthError::MissingCredentials,
        || AuthError::InvalidSignature,
        || AuthError::Expired,
        || AuthError::failed("refused"),
        || AuthError::unavailable("store unreachable", None),
    ];
    for outcome in outcomes {
        let guard = AuthnGuard::new(Arc::new(RejectWith(outcome)));
        for (site, public) in [(&GATED, false), (&PUBLIC, true)] {
            if let Err(denial) = check(&guard, site, &mut bare()).await {
                let declared = guard.denial_statuses(public);
                assert!(
                    declared.contains(&denial.http_status()),
                    "{:?} on a route (public: {public}) answers {}, outside the declared \
                     {declared:?}",
                    outcome(),
                    denial.http_status(),
                );
            }
        }
    }
}

#[tokio::test]
async fn unreachable_store_fails_closed_even_on_a_public_route() {
    // The credential was never evaluated, so serving the caller as anonymous
    // would silently downgrade every authenticated session during an outage.
    let logs = LogCapture::install();
    let guard = AuthnGuard::new(Arc::new(RejectWith(|| {
        AuthError::unavailable("store unreachable", None)
    })));

    let denial = check(&guard, &PUBLIC, &mut bare())
        .await
        .expect_err("an unevaluated credential must not pass as anonymous");
    assert_eq!(denial.retry_after_secs(), None, "{denial:?}");
    assert_eq!(denial.http_status(), 503, "the caller did nothing wrong");

    // The client message is opaque, so the outage is only readable here.
    let event = logs.expect_one(nest_rs_authn::TARGET, DENIED);
    assert_eq!(
        event.level, "warn",
        "a 503 is filed at warn, as every edge files one"
    );
    assert_eq!(event.field("reason").as_deref(), Some("unavailable"));
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("store unreachable")),
        "the event carries the underlying failure, got {:?}",
        event.fields,
    );
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

/// A principal acting within a tenant, as a SaaS product's claims are.
#[derive(Clone, Debug)]
struct Member(&'static str);

impl AmbientValue for Member {}

impl PrincipalIdentity for Member {
    fn actor_id(&self) -> Option<String> {
        Some("ada".to_owned())
    }

    fn tenant_id(&self) -> Option<String> {
        Some(self.0.to_owned())
    }
}

struct AuthenticateMember(&'static str);

#[async_trait]
impl Strategy for AuthenticateMember {
    type Principal = Member;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Ok(Member(self.0))
    }
}

/// The tenant ambient for what runs after `guard` passed `req` on `site`.
async fn tenant_after(
    guard: &dyn Guard,
    site: &'static Handler,
    mut req: nest_rs_http::Request,
) -> Option<String> {
    check(guard, site, &mut req)
        .await
        .expect("the guard passes");
    install_pending(attached_pending(&req), async {
        nest_rs_core::current_tenant_id()
    })
    .await
}

#[tokio::test]
async fn a_principal_naming_a_tenant_makes_it_ambient_for_the_request() {
    let guard = AuthnGuard::new(Arc::new(AuthenticateMember("acme")));
    assert_eq!(
        tenant_after(&guard, &GATED, bare()).await.as_deref(),
        Some("acme"),
    );
    assert_eq!(
        nest_rs_core::current_tenant_id(),
        None,
        "nothing is ambient before the guard's stage is installed",
    );
}

#[tokio::test]
async fn a_principal_naming_no_tenant_leaves_the_request_without_one() {
    let unscoped = AuthnGuard::new(Arc::new(AuthenticateAs("alice")));
    assert_eq!(tenant_after(&unscoped, &GATED, bare()).await, None);
    let empty = AuthnGuard::new(Arc::new(AuthenticateMember("")));
    assert_eq!(
        tenant_after(&empty, &GATED, bare()).await,
        None,
        "an empty id names no tenant",
    );
}

/// The tenant is an authorization input: what the caller sends beside its
/// credential names none, authenticated or not.
#[tokio::test]
async fn a_header_named_like_the_tenant_sets_none() {
    let claiming = || {
        nest_rs_http::Request::builder()
            .header(
                nest_rs_http::HeaderName::from_static("x-tenant-id"),
                nest_rs_http::HeaderValue::from_static("acme"),
            )
            .header(
                nest_rs_http::HeaderName::from_static("tenant_id"),
                nest_rs_http::HeaderValue::from_static("acme"),
            )
            .finish()
    };
    let authenticated = AuthnGuard::new(Arc::new(AuthenticateAs("alice")));
    assert_eq!(tenant_after(&authenticated, &GATED, claiming()).await, None);
    let anonymous = AuthnGuard::new(Arc::new(FailWith));
    assert_eq!(tenant_after(&anonymous, &PUBLIC, claiming()).await, None);
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

/// The line every denial of a request is filed as, on the guard's target.
const DENIED: &str = "guard denied the request";

/// The reason a strategy that did not answer is filed under — the one thing
/// that tells this denial from an unreachable store's, which answers the
/// caller the same way.
const STALLED: &str = "strategy_timeout";

/// What every handler below answers, so a response carrying it is a request the
/// guard let through.
const REACHED: &str = "reached";

/// The refusal alone reads like an outage; this line says the strategy never
/// answered, which strategy, on which edge, and how long it was waited on.
fn assert_the_strategy_was_waited_out(logs: &LogCapture) {
    let cause = logs.expect_one(nest_rs_authn::TARGET, DENIED);
    assert_eq!(cause.level, "warn");
    assert_eq!(cause.field("reason").as_deref(), Some(STALLED));
    assert_eq!(cause.field("unit").as_deref(), Some("http.request"));
    let chain = cause.field("error").unwrap_or_default();
    assert!(
        chain.contains("the strategy `StalledStrategy` did not answer"),
        "the line names the strategy, got {:?}",
        cause.fields,
    );
    assert!(
        chain.contains(&format!("{} ms", AUTHENTICATE_TIMEOUT.as_millis())),
        "…and how long it was waited on, got {chain}",
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
    let unavailable = AuthError::unavailable("", None);
    assert_eq!(denial.http_status(), 503, "{denial:?}");
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
        assert_eq!(denial.http_status(), 503, "{denial:?}");
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
        Some("PanickingStrategy"),
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

    assert_eq!(denial.http_status(), 503, "{denial:?}");
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
    logs.expect_none(nest_rs_authn::TARGET, DENIED);
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
    let held = logs
        .find(nest_rs_authn::TARGET, DENIED)
        .into_iter()
        .filter(|line| line.field("reason").as_deref() == Some(STALLED))
        .count();
    assert_eq!(held, 2, "one line per request the strategy held");
}

/// The sender a signed webhook authenticates as.
#[derive(Clone, Debug)]
struct Sender(String);

impl AmbientValue for Sender {}

impl PrincipalIdentity for Sender {
    fn actor_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// The mark a signed delivery's body opens with.
const SIGNED: &[u8] = b"signed:";

/// Authenticates a webhook delivery as the path's `{sender}` when its body is
/// signed, reading the body the edge holds and setting it back for the handler.
#[injectable]
#[derive(Default)]
struct WebhookStrategy;

#[async_trait]
impl Strategy for WebhookStrategy {
    type Principal = Sender;

    async fn authenticate(&self, req: &mut Request) -> Result<Sender, AuthError> {
        let sender = req
            .path_param("sender")
            .ok_or(AuthError::MissingCredentials)?
            .to_owned();
        let Ok(body) = req.take_body().into_bytes().await else {
            return Err(AuthError::MissingCredentials);
        };
        let signed = body.starts_with(SIGNED);
        req.set_body(body);
        if signed {
            Ok(Sender(sender))
        } else {
            Err(AuthError::InvalidSignature)
        }
    }
}

type WebhookAuthn = AuthnGuard<WebhookStrategy>;

#[controller(path = "/hooks")]
#[use_guards(WebhookAuthn)]
struct HooksController;

#[routes]
impl HooksController {
    #[post("/{sender}")]
    async fn deliver(&self, sender: Ambient<Sender>, body: RawBody) -> String {
        format!(
            "{} sent {}",
            sender.0.0,
            String::from_utf8_lossy(&body.into_inner())
        )
    }
}

#[module(
    imports = [HttpModule::for_root(edge())],
    providers = [WebhookStrategy, WebhookAuthn, HooksController],
)]
struct HooksModule;

/// A strategy reads the request the edge holds — the route's parameters, the
/// body — and what it sets back is what the handler reads.
#[tokio::test]
async fn a_strategy_reads_the_routed_request_and_leaves_its_body_to_the_handler() {
    let app = TestApp::for_module::<HooksModule>()
        .await
        .expect("a controller binding the guard boots");

    let delivered = app
        .http()
        .post("/hooks/billing")
        .body("signed:invoice paid")
        .send()
        .await;
    delivered.assert_status(StatusCode::OK);
    let body = delivered.0.into_body().into_string().await.expect("a body");
    assert_eq!(body, "billing sent signed:invoice paid");

    app.http()
        .post("/hooks/billing")
        .body("invoice paid")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// Authenticates `Bearer good`, refuses any other bearer, and reports a
/// request carrying none as missing credentials.
#[injectable]
#[derive(Default)]
struct BearerStrategy;

#[async_trait]
impl Strategy for BearerStrategy {
    type Principal = Who;

    async fn authenticate(&self, req: &mut Request) -> Result<Self::Principal, AuthError> {
        match req.headers().get(nest_rs_http::header::AUTHORIZATION) {
            None => Err(AuthError::MissingCredentials),
            Some(bearer) if bearer == "Bearer good" => Ok(Who("ada")),
            Some(_) => Err(AuthError::InvalidToken),
        }
    }
}

type BearerAuthn = AuthnGuard<BearerStrategy>;

mod graphql {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use nest_rs_core::{Layer, UnitContext, injectable, module};
    use nest_rs_graphql::async_graphql::{Result as GraphqlResult, SimpleObject};
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
    use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase};
    use nest_rs_http::{EndpointGuards, HeaderName, HttpModule, HttpView, StatusCode, header};
    use nest_rs_testing::{LogCapture, TestApp, TestResponse};

    use super::{
        BearerAuthn, BearerStrategy, REACHED, StalledAuthn, StalledStrategy,
        assert_refused_at_the_bound, assert_the_strategy_was_waited_out, edge,
        within_twice_the_bound,
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
        imports = [HttpModule::for_root(edge()), GraphqlModule],
        providers = [StalledStrategy, StalledAuthn, TickResolver],
    )]
    struct StalledGraphqlModule;

    #[module(
        imports = [StalledGraphqlModule],
        providers = [nest_rs_guards::GlobalGuards<(StalledAuthn,)>],
    )]
    struct TheGraphqlPostIsRefusedAtTheBoundApp;

    /// The GraphQL POST is authenticated by the global pool, in band: an
    /// anonymous caller reaches the resolvers' own gates. A strategy that never
    /// answered is not that caller, and the POST is refused at the bound rather
    /// than served anonymously.
    #[tokio::test(start_paused = true)]
    async fn the_graphql_post_is_refused_at_the_bound() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<TheGraphqlPostIsRefusedAtTheBoundApp>()
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

    #[module(
        imports = [GraphqlModule],
        providers = [BearerStrategy, BearerAuthn, TickResolver],
    )]
    struct BearerGraphqlModule;

    #[module(
        imports = [BearerGraphqlModule],
        providers = [nest_rs_guards::GlobalGuards<(BearerAuthn,)>],
    )]
    struct TheGraphqlPostAdmitsAnAnonymousCallerAndRefusesABadBearerApp;

    /// A subgraph: `Widget`'s `@key` is what its `#[entity]` takes, and `#[public]`
    /// says what an anonymous reference may reach once the fields a router calls
    /// let it through.
    #[derive(SimpleObject)]
    #[graphql(crate = "::nest_rs_graphql::async_graphql")]
    struct Widget {
        id: i32,
    }

    #[resolver]
    struct WidgetsResolver;

    #[operations]
    impl WidgetsResolver {
        #[entity]
        #[public]
        async fn find_widget_by_id(&self, id: i32) -> GraphqlResult<Widget> {
            Ok(Widget { id })
        }
    }

    fn subgraph() -> nest_rs_graphql::GraphqlSetup {
        GraphqlModule::for_root(GraphqlConfig {
            federation: true,
            ..GraphqlConfig::default()
        })
    }

    #[module(
        imports = [subgraph()],
        providers = [BearerStrategy, BearerAuthn, WidgetsResolver],
    )]
    struct BearerSubgraphModule;

    #[module(
        imports = [BearerSubgraphModule],
        providers = [nest_rs_guards::GlobalGuards<(BearerAuthn,)>],
    )]
    struct PooledSubgraphApp;

    #[module(
        imports = [BearerSubgraphModule],
        providers = [EndpointGuards<GraphqlView, (BearerAuthn,)>],
    )]
    struct EndpointGuardedSubgraphApp;

    const SDL: &str = "{ _service { sdl } }";
    const REFERENCE: &str = r#"{ _entities(representations: [{ __typename: "Widget", id: 7 }]) { ... on Widget { id } } }"#;

    /// `query` over `/graphql`, under `bearer` when one is presented.
    async fn subgraph_answer(
        app: &TestApp,
        query: &str,
        bearer: Option<&str>,
    ) -> serde_json::Value {
        let mut request = app.http().post("/graphql");
        if let Some(bearer) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        let answered = request
            .body_json(&serde_json::json!({ "query": query }))
            .send()
            .await;
        answered.assert_status(StatusCode::OK);
        answered.json().await.value().deserialize()
    }

    /// `body` refused for want of a principal, as every gated unit is.
    fn assert_anonymous_refused(body: &serde_json::Value, field: &str) {
        assert!(body["data"][field].is_null(), "{field} answered: {body}");
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
            "{body}",
        );
    }

    #[resolver]
    struct WhoamiResolver;

    #[operations]
    impl WhoamiResolver {
        #[query]
        #[public]
        async fn whoami(&self, who: nest_rs_core::Ambient<super::Who>) -> GraphqlResult<String> {
            Ok(who.into_inner().0.to_owned())
        }
    }

    #[module(
        imports = [GraphqlModule],
        providers = [BearerStrategy, BearerAuthn, WhoamiResolver],
    )]
    struct WhoamiModule;

    #[module(
        imports = [WhoamiModule],
        providers = [nest_rs_guards::GlobalGuards<(BearerAuthn,)>],
    )]
    struct WhoamiApp;

    /// In band, the guard admits a `/graphql` request carrying no credential:
    /// a `#[public]` field reading the principal as an `Ambient` refuses that
    /// caller `UNAUTHENTICATED`, never the wiring error an unattached value is.
    #[tokio::test]
    async fn a_public_field_reading_its_principal_refuses_a_caller_with_no_credential() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<WhoamiApp>().await.expect("boots");

        let named = subgraph_answer(&app, "{ whoami }", Some("good")).await;
        assert_eq!(named["data"]["whoami"], "ada", "{named}");

        let anonymous = subgraph_answer(&app, "{ whoami }", None).await;
        assert_anonymous_refused(&anonymous, "whoami");
        let line = logs.expect_one(nest_rs_authn::TARGET, "guard denied the operation");
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("reason").as_deref(), Some("anonymous_caller"));
        assert!(
            logs.find("nest_rs::graphql", "graphql operation failed")
                .is_empty(),
            "a caller with no credential is no wiring error",
        );
    }

    #[tokio::test]
    async fn an_anonymous_caller_is_refused_the_sdl_under_a_pooled_authn_guard() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<PooledSubgraphApp>()
            .await
            .expect("a subgraph under a pooled AuthnGuard boots");

        let anonymous = subgraph_answer(&app, SDL, None).await;
        assert_anonymous_refused(&anonymous, "_service");
        let line = logs.expect_one(nest_rs_authn::TARGET, "guard denied the federation field");
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("field").as_deref(), Some("_service"));
        assert_eq!(line.field("reason").as_deref(), Some("anonymous_caller"));

        let router = subgraph_answer(&app, SDL, Some("good")).await;
        assert!(
            router["data"]["_service"]["sdl"]
                .as_str()
                .is_some_and(|sdl| sdl.contains("Widget")),
            "a caller the guard identified reads it: {router}",
        );
    }

    /// The reference composition lists no pool: `/graphql`'s `EndpointGuards` admit
    /// the anonymous request, and `_service` states no posture to refuse it with.
    #[tokio::test]
    async fn an_anonymous_caller_is_refused_the_sdl_under_endpoint_guards() {
        let app = TestApp::for_module::<EndpointGuardedSubgraphApp>()
            .await
            .expect("a subgraph under EndpointGuards boots");

        let anonymous = subgraph_answer(&app, SDL, None).await;
        assert_anonymous_refused(&anonymous, "_service");

        let router = subgraph_answer(&app, SDL, Some("good")).await;
        assert!(
            router["data"]["_service"]["sdl"].is_string(),
            "a caller the guard identified reads it: {router}",
        );
    }

    #[tokio::test]
    async fn an_anonymous_reference_is_refused_under_a_pooled_authn_guard() {
        let app = TestApp::for_module::<PooledSubgraphApp>()
            .await
            .expect("a subgraph under a pooled AuthnGuard boots");

        let anonymous = subgraph_answer(&app, REFERENCE, None).await;
        assert_anonymous_refused(&anonymous, "_entities");

        let router = subgraph_answer(&app, REFERENCE, Some("good")).await;
        assert_eq!(router["data"]["_entities"][0]["id"], 7, "{router}");
    }

    /// `EndpointGuards` add to the request alone: each `#[entity]` states its own
    /// posture, so what this one opens stays open.
    #[tokio::test]
    async fn an_anonymous_reference_reaches_a_public_entity_under_endpoint_guards() {
        let app = TestApp::for_module::<EndpointGuardedSubgraphApp>()
            .await
            .expect("a subgraph under EndpointGuards boots");

        let anonymous = subgraph_answer(&app, REFERENCE, None).await;
        assert_eq!(anonymous["data"]["_entities"][0]["id"], 7, "{anonymous}");
    }

    /// `x-router-key` on the request `cx` dispatches: a `500` on a unit with no request.
    fn keyed(cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        match HttpView::of(cx)?.request().header("x-router-key") {
            Some(_) => Ok(()),
            None => Err(Denial::unauthorized("missing router key")),
        }
    }

    /// A router's key, decided at the request, the units it carries admitted.
    #[injectable]
    #[derive(Layer)]
    struct RouterKey;

    #[async_trait]
    impl Guard for RouterKey {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<HttpView> for RouterKey {}

    /// The same key, read in `check`, the one entry a guard on a route writes.
    #[injectable]
    #[derive(Layer)]
    struct RouterKeyInCheck;

    #[async_trait]
    impl Guard for RouterKeyInCheck {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }

        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }
    }

    impl GuardFor<HttpView> for RouterKeyInCheck {}

    /// How many requests [`RateLimit`] charged, once each by the process's one test.
    static CHARGED: AtomicUsize = AtomicUsize::new(0);

    /// A rate limit reading the request in `check`, identifying no one.
    #[injectable]
    #[derive(Layer)]
    struct RateLimit;

    #[async_trait]
    impl Guard for RateLimit {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            HttpView::of(cx)?;
            CHARGED.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl GuardFor<HttpView> for RateLimit {}

    #[module(
        imports = [subgraph()],
        providers = [RouterKey, WidgetsResolver, EndpointGuards<GraphqlView, (RouterKey,)>],
    )]
    struct RouterKeySubgraphApp;

    #[module(
        imports = [subgraph()],
        providers = [
            RouterKeyInCheck,
            WidgetsResolver,
            EndpointGuards<GraphqlView, (RouterKeyInCheck,)>,
        ],
    )]
    struct RouterKeyInCheckSubgraphApp;

    #[module(
        imports = [BearerSubgraphModule],
        providers = [RateLimit, EndpointGuards<GraphqlView, (RateLimit, BearerAuthn)>],
    )]
    struct RateLimitedSubgraphApp;

    #[module(
        imports = [subgraph()],
        providers = [RateLimit, WidgetsResolver, EndpointGuards<GraphqlView, (RateLimit,)>],
    )]
    struct RateLimitAloneSubgraphApp;

    /// `SDL` over `/graphql`, under `key` when the router presents one.
    async fn read_sdl_with_key(app: &TestApp, key: Option<&str>) -> TestResponse {
        let mut request = app.http().post("/graphql");
        if let Some(key) = key {
            request = request.header(HeaderName::from_static("x-router-key"), key);
        }
        request
            .body_json(&serde_json::json!({ "query": SDL }))
            .send()
            .await
    }

    #[tokio::test]
    async fn a_listed_guard_deciding_at_the_request_lets_its_router_read_the_sdl() {
        let app = TestApp::for_module::<RouterKeySubgraphApp>()
            .await
            .expect("a subgraph under EndpointGuards boots");

        let keyed = read_sdl_with_key(&app, Some("k")).await;
        keyed.assert_status(StatusCode::OK);
        let body: serde_json::Value = keyed.json().await.value().deserialize();
        assert!(
            body["data"]["_service"]["sdl"].is_string(),
            "the router presented its key: {body}",
        );

        read_sdl_with_key(&app, None)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    /// A guard saying it identifies its caller is asked on `_service`, whose unit holds
    /// no request: it fails closed for every caller, and its line says the edit.
    #[tokio::test]
    async fn a_listed_guard_reading_the_request_in_check_fails_closed_on_the_sdl() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<RouterKeyInCheckSubgraphApp>()
            .await
            .expect("a subgraph under EndpointGuards boots");

        let refused = read_sdl_with_key(&app, Some("k")).await;
        refused.assert_status(StatusCode::OK);
        let body: serde_json::Value = refused.json().await.value().deserialize();
        assert!(body["data"].is_null(), "no SDL past a failed guard: {body}");
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "INTERNAL",
            "{body}"
        );

        let denial = logs.expect_one("nest_rs::layers", "guard denied the federation field");
        assert_eq!(denial.field("guard").as_deref(), Some("RouterKeyInCheck"));
        assert!(
            denial
                .field("error")
                .is_some_and(|chain| chain.contains("decide the request in `check_http`")),
            "the edge's line says the edit: {denial:?}",
        );
        logs.expect_none(
            "nest_rs::http",
            "a guard read the HTTP request of a unit with no HTTP view",
        );
    }

    /// A rate limit decided what it decides at the request: asked again, it would read a
    /// request `_service` does not hold, and charge the caller twice. The guard beside it
    /// that identifies a caller is still asked.
    #[tokio::test]
    async fn a_listed_guard_identifying_no_one_is_not_asked_again_on_the_sdl() {
        let app = TestApp::for_module::<RateLimitedSubgraphApp>()
            .await
            .expect("a subgraph under EndpointGuards boots");

        let router = subgraph_answer(&app, SDL, Some("good")).await;
        assert!(
            router["data"]["_service"]["sdl"].is_string(),
            "the rate limit admitted it at the request: {router}",
        );
        assert_eq!(CHARGED.load(Ordering::SeqCst), 1, "one request, one charge");

        let anonymous = subgraph_answer(&app, SDL, None).await;
        assert_anonymous_refused(&anonymous, "_service");
    }

    /// With no guard identifying anyone, `federation: true` is the line that opens the SDL.
    #[tokio::test]
    async fn a_subgraph_whose_guards_identify_no_one_serves_the_sdl_past_them() {
        let app = TestApp::for_module::<RateLimitAloneSubgraphApp>()
            .await
            .expect("a subgraph under EndpointGuards boots");

        let anonymous = subgraph_answer(&app, SDL, None).await;
        assert!(
            anonymous["data"]["_service"]["sdl"].is_string(),
            "{anonymous}"
        );
    }

    /// `/graphql` is in band: an anonymous caller reaches a `#[public]` query,
    /// while a bearer the caller presented is still verified, and a bad one is
    /// refused before any operation runs.
    #[tokio::test]
    async fn the_graphql_post_admits_an_anonymous_caller_and_refuses_a_bad_bearer() {
        let app = TestApp::builder()
            .module::<TheGraphqlPostAdmitsAnAnonymousCallerAndRefusesABadBearerApp>()
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

    use nest_rs_http::HttpModule;
    use nest_rs_http::StatusCode;
    use nest_rs_mcp::{McpError, mcp, tools};
    use nest_rs_testing::mcp::{call_tool, initialize_request};
    use nest_rs_testing::{LogCapture, TestApp};

    use super::{
        BearerAuthn, BearerStrategy, REACHED, StalledAuthn, StalledStrategy,
        assert_refused_at_the_bound, assert_the_strategy_was_waited_out, edge,
        within_twice_the_bound,
    };

    #[mcp]
    #[derive(Clone, Default)]
    struct TickTool;

    #[tools]
    impl TickTool {
        /// Answer with a constant — the assertions are about what ran around it.
        #[tool]
        async fn tick(&self) -> Result<String, McpError> {
            Ok(REACHED.to_owned())
        }
    }

    #[module(
        imports = [HttpModule::for_root(edge())],
        providers = [StalledStrategy, StalledAuthn, TickTool],
    )]
    struct StalledMcpModule;

    #[module(
        imports = [StalledMcpModule],
        providers = [nest_rs_guards::GlobalGuards<(StalledAuthn,)>],
    )]
    struct TheMcpPostIsRefusedAtTheBoundApp;

    /// The MCP POST is authenticated in the endpoint's `before`, by the global
    /// pool run in band, ahead of any operation — so the bound refuses the
    /// request before rmcp has opened a stream.
    #[tokio::test(start_paused = true)]
    async fn the_mcp_post_is_refused_at_the_bound() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<TheMcpPostIsRefusedAtTheBoundApp>()
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

    #[module(providers = [
        BearerStrategy,
        BearerAuthn,
        TickTool,
        nest_rs_guards::GlobalGuards<(BearerAuthn,)>,
    ])]
    struct PooledBearerApp;

    /// The operation runs on rmcp's task, so the pool's unit check reads the
    /// principal the `/mcp` request attached across it.
    #[tokio::test]
    async fn a_pooled_guard_admits_the_operation_its_request_identified() {
        let app = TestApp::for_module::<PooledBearerApp>()
            .await
            .expect("an #[mcp] host under a pooled AuthnGuard boots");

        let answered = call_tool(
            app.http(),
            nest_rs_mcp::DEFAULT_PATH,
            "tick",
            Some("Bearer good"),
        )
        .await;
        assert!(answered.contains(REACHED), "{answered}");
    }
}

mod ws {
    use nest_rs_core::module;
    use nest_rs_http::HttpModule;
    use nest_rs_http::{StatusCode, header};
    use nest_rs_testing::{LogCapture, TestApp};
    use nest_rs_ws::{WsModule, gateway, messages};

    use super::{
        BearerAuthn, BearerStrategy, REACHED, StalledAuthn, StalledStrategy,
        assert_refused_at_the_bound, assert_the_strategy_was_waited_out, edge,
        within_twice_the_bound,
    };

    const PATH: &str = "/ws/stalled";

    #[gateway(path = "/ws/stalled")]
    #[use_guards(StalledAuthn)]
    struct StalledGateway;

    #[messages]
    impl StalledGateway {
        #[subscribe_message("tick")]
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

        // Filed once, on the guard's target: one denial, one line.
        logs.expect_none(nest_rs_core::target::LAYERS, super::DENIED);
    }

    #[gateway(path = "/ws/bearer")]
    struct BearerGateway;

    #[messages]
    impl BearerGateway {
        #[subscribe_message("tick")]
        async fn tick(&self) -> &'static str {
            REACHED
        }
    }

    #[module(
        imports = [WsModule],
        providers = [
            BearerStrategy,
            BearerAuthn,
            BearerGateway,
            nest_rs_guards::GlobalGuards<(BearerAuthn,)>,
        ],
    )]
    struct PooledBearerApp;

    /// A message is a unit of its own, so the pool's unit check reads the
    /// principal the upgrade attached across the connection.
    #[tokio::test]
    async fn a_pooled_guard_admits_the_message_its_upgrade_identified() {
        let app = TestApp::builder()
            .module::<PooledBearerApp>()
            .build_ws()
            .await
            .expect("a gateway under a pooled AuthnGuard boots");

        let mut socket = app
            .socket("/ws/bearer")
            .header("authorization", "Bearer good")
            .connect()
            .await;
        socket.send("tick", serde_json::Value::Null).await;
        let answered = socket.next_envelope().await;
        assert_eq!(answered["data"], REACHED, "{answered}");

        app.shutdown().await.expect("the transport stops cleanly");
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
    use nest_rs_core::__private::provide_meta;
    use nest_rs_core::{App, Budget, BudgetPastNetError, injectable, module};
    use nest_rs_http::Request;

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
        let Err(refused) = provide_meta(
            App::builder().provide(IdentityStore(AUTHENTICATE_TIMEOUT)),
            store_budget(),
        )
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
        provide_meta(
            App::builder().provide(IdentityStore(AUTHENTICATE_TIMEOUT * 2)),
            store_budget(),
        )
        .module::<LocalModule>()
        .build()
        .await
        .expect("a strategy reaching no store boots beside a slow one");
    }
}

/// A credential a `#[public]` route set aside earns its `401` wherever the
/// principal is read: a parameter, or a guard requiring it (RFC 6750 §3).
mod rejected_credential {
    use nest_rs_core::{Layer, UnitContext};
    use nest_rs_guards::{GuardFor, PrincipalClaim};
    use nest_rs_http::header;

    use super::*;

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

    #[injectable]
    #[derive(Default)]
    struct NeedsWho;

    impl Layer for NeedsWho {}

    #[async_trait]
    impl Guard for NeedsWho {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            cx.require::<super::Who>()?;
            Ok(())
        }

        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<super::Who>())
        }
    }

    impl GuardFor<nest_rs_http::HttpView> for NeedsWho {}

    #[controller(path = "/rejected")]
    #[use_guards(BearerAuthn)]
    struct RejectedController;

    #[routes]
    impl RejectedController {
        #[get("/guarded")]
        #[public]
        #[use_guards(NeedsWho)]
        async fn guarded(&self) -> &'static str {
            "reached"
        }

        #[get("/parameter")]
        #[public]
        async fn parameter(&self, _who: Ambient<super::Who>) -> &'static str {
            "reached"
        }
    }

    #[module(
        imports = [HttpModule],
        providers = [BearerStrategy, BearerAuthn, NeedsWho, RejectedController],
    )]
    struct RejectedModule;

    /// What `path` answers `credential`: its status, its challenge and its body.
    async fn answer(
        app: &TestApp,
        path: &str,
        credential: Option<&str>,
    ) -> (StatusCode, String, serde_json::Value) {
        let mut request = app.http().get(path);
        if let Some(credential) = credential {
            request = request.header(header::AUTHORIZATION, credential);
        }
        let response = request.send().await.0;
        let status = response.status();
        let challenge = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response.into_body().into_string().await.expect("a body");
        let body = serde_json::from_str(&body).unwrap_or(serde_json::Value::String(body));
        (status, challenge, body)
    }

    #[tokio::test]
    async fn a_guard_requiring_the_principal_answers_the_401_the_credential_earned() {
        let app = TestApp::for_module::<RejectedModule>()
            .await
            .expect("boots");
        let logs = LogCapture::install();

        let (status, challenge, body) =
            answer(&app, "/rejected/guarded", Some("Bearer forged")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert!(challenge.contains("invalid_token"), "{challenge}");
        assert_eq!(body["detail"], "invalid token", "{body}");
        let filed = logs.expect_one(nest_rs_core::target::LAYERS, "guard denied the request");
        assert_eq!(
            filed.field("reason").as_deref(),
            Some("rejected_credential")
        );
    }

    #[tokio::test]
    async fn the_same_401_answers_a_parameter_and_a_guard_reading_the_principal() {
        let app = TestApp::for_module::<RejectedModule>()
            .await
            .expect("boots");

        let by_guard = answer(&app, "/rejected/guarded", Some("Bearer forged")).await;
        let by_parameter = answer(&app, "/rejected/parameter", Some("Bearer forged")).await;

        assert_eq!(by_guard, by_parameter);
    }

    #[tokio::test]
    async fn a_caller_with_no_credential_reads_as_one_that_sent_none() {
        let app = TestApp::for_module::<RejectedModule>()
            .await
            .expect("boots");

        let (status, challenge, body) = answer(&app, "/rejected/guarded", None).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(
            challenge, "Bearer",
            "RFC 6750 §3: no reason for no credential"
        );
        assert_eq!(body["detail"], "unauthenticated", "{body}");
    }
}

/// A token bound to a key reaches the caller as any refused token, and the
/// operator as what it is, on the guard's one line.
mod sender_constrained {
    use std::sync::Arc;

    use nest_rs_authn::__private::sign_unchecked;
    use nest_rs_authn::{JwtService, JwtStrategy};
    use nest_rs_core::Ambient;
    use nest_rs_http::{StatusCode, controller, header, routes};
    use nest_rs_testing::{LogCapture, TestApp};
    use serde::{Deserialize, Serialize};

    use super::*;

    const SECRET: &str = "sender-constrained-secret-padded-32";

    #[derive(Clone, Debug, Serialize, Deserialize, PrincipalIdentity)]
    struct Claims {
        #[principal(actor)]
        sub: String,
    }

    type Authenticated = AuthnGuard<JwtStrategy<Claims>>;

    #[controller(path = "/")]
    #[use_guards(Authenticated)]
    struct ProfileController;

    #[routes]
    impl ProfileController {
        #[get("/me")]
        async fn me(&self, claims: Ambient<Claims>) -> String {
            claims.sub.clone()
        }
    }

    #[module(
        imports = [HttpModule, nest_rs_authn::AuthnModule::for_root(nest_rs_authn::AuthnConfig {
            secret: Some(SECRET.into()),
            ..nest_rs_authn::AuthnConfig::default()
        })],
        providers = [JwtStrategy<Claims>, Authenticated, ProfileController],
    )]
    struct ProfileApp;

    #[tokio::test]
    async fn a_bound_token_is_a_401_invalid_token_on_one_warn_naming_it() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<ProfileApp>().await.expect("boots");
        let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");
        let bound = sign_unchecked(
            &jwt,
            &serde_json::json!({ "sub": "alice", "cnf": { "jkt": "thumbprint" } }),
        )
        .expect("sign");

        let response = app
            .http()
            .get("/me")
            .header("authorization", format!("Bearer {bound}"))
            .send()
            .await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        let challenge = response.0.headers()[header::WWW_AUTHENTICATE]
            .to_str()
            .expect("ASCII");
        assert!(
            challenge.contains(r#"error="invalid_token""#),
            "{challenge}"
        );
        assert!(!challenge.contains("cnf"), "{challenge}");
        let warns = logs.events_at(tracing::Level::WARN);
        assert_eq!(warns.len(), 1, "{warns:#?}");
        assert_eq!(
            warns[0].field("reason").as_deref(),
            Some("sender_constrained")
        );

        let unbound = sign_unchecked(&jwt, &serde_json::json!({ "sub": "alice" })).expect("sign");
        app.http()
            .get("/me")
            .header("authorization", format!("Bearer {unbound}"))
            .send()
            .await
            .assert_status_is_ok();
    }
}
