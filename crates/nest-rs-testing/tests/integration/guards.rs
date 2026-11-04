//! Guard effectiveness across the three scopes — **handler**, **controller**
//! and **global** — plus multi-guard ordering and `#[public]` routes.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Ambient, AmbientValue, Layer, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::StatusCode;
use nest_rs_http::{HttpView, controller, routes};
use nest_rs_testing::{HeadlessApp, TestApp};
use tokio::sync::Mutex;

use crate::refused_wiring;

/// Denies every request with `403 Forbidden`.
#[injectable]
#[derive(Default)]
struct DenyGuard;

impl Layer for DenyGuard {}

#[async_trait]
impl Guard for DenyGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("forbidden"))
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for DenyGuard {}

/// Denies with `401 Unauthorized` — paired with [`DenyGuard`] to observe order.
#[injectable]
#[derive(Default)]
struct ChallengeGuard;

impl Layer for ChallengeGuard {}

#[async_trait]
impl Guard for ChallengeGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::unauthorized("unauthorized"))
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for ChallengeGuard {}

#[controller(path = "/h")]
struct HandlerScope;

#[routes]
impl HandlerScope {
    #[get("/guarded")]
    #[use_guards(DenyGuard)]
    async fn guarded(&self) -> &'static str {
        "unreachable"
    }

    #[get("/open")]
    #[public]
    async fn open(&self) -> &'static str {
        "ok"
    }
}

#[controller(path = "/c")]
#[use_guards(DenyGuard)]
struct ControllerScope;

#[routes]
impl ControllerScope {
    #[get("/one")]
    async fn one(&self) -> &'static str {
        "unreachable"
    }

    #[get("/two")]
    async fn two(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [DenyGuard, HandlerScope, ControllerScope])]
struct ScopeModule;

#[tokio::test]
async fn guard_on_a_handler_protects_only_that_route() {
    let app = TestApp::for_module::<ScopeModule>().await.expect("boots");

    app.http()
        .get("/h/guarded")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    app.http().get("/h/open").send().await.assert_status_is_ok();
}

#[tokio::test]
async fn guard_on_a_controller_protects_every_route() {
    let app = TestApp::for_module::<ScopeModule>().await.expect("boots");

    for path in ["/c/one", "/c/two"] {
        app.http()
            .get(path)
            .send()
            .await
            .assert_status(StatusCode::FORBIDDEN);
    }
}

#[controller(path = "/g")]
struct PublicEverywhere;

#[routes]
impl PublicEverywhere {
    #[get("/a")]
    #[public]
    async fn a(&self) -> &'static str {
        "ok"
    }

    #[get("/b")]
    #[public]
    async fn b(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [DenyGuard, PublicEverywhere])]
struct PublicModule;

#[module(imports = [PublicModule], providers = [GlobalGuards<(DenyGuard,)>])]
struct AGlobalGuardProtectsEveryRouteWithoutUseGuardsApp;

#[tokio::test]
async fn a_global_guard_protects_every_route_without_use_guards() {
    let app = TestApp::builder()
        .module::<AGlobalGuardProtectsEveryRouteWithoutUseGuardsApp>()
        .build()
        .await
        .expect("boots with a global guard");

    for path in ["/g/a", "/g/b"] {
        app.http()
            .get(path)
            .send()
            .await
            .assert_status(StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn without_a_global_guard_the_same_routes_stay_open() {
    let app = TestApp::builder()
        .module::<PublicModule>()
        .build()
        .await
        .expect("boots");

    for path in ["/g/a", "/g/b"] {
        app.http().get(path).send().await.assert_status_is_ok();
    }
}

// A denying guard cannot prove it ran once, so the dedup tests count and admit.

/// One process-global counter shared by the two counting tests, so they are
/// serialized behind [`GATE`] to keep their reads deterministic.
static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

/// Counts every execution, then admits — the count, not the status, is asserted.
#[injectable]
#[derive(Default)]
struct CountingGuard;

impl Layer for CountingGuard {}

#[async_trait]
impl Guard for CountingGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for CountingGuard {}

#[controller(path = "/dedup")]
#[use_guards(CountingGuard)]
struct DedupController;

#[routes]
impl DedupController {
    #[get("/c-only")]
    async fn c_only(&self) -> &'static str {
        "ok"
    }

    #[get("/c-and-m")]
    #[use_guards(CountingGuard)]
    async fn c_and_m(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [CountingGuard, DedupController])]
struct DedupModule;

#[tokio::test]
async fn the_same_guard_at_controller_and_method_scope_executes_once() {
    let _gate = GATE.lock().await;
    let app = TestApp::for_module::<DedupModule>().await.expect("boots");

    COUNTER.store(0, Ordering::SeqCst);
    app.http()
        .get("/dedup/c-only")
        .send()
        .await
        .assert_status_is_ok();
    assert_eq!(
        COUNTER.load(Ordering::SeqCst),
        1,
        "a controller-scope guard runs exactly once",
    );

    COUNTER.store(0, Ordering::SeqCst);
    app.http()
        .get("/dedup/c-and-m")
        .send()
        .await
        .assert_status_is_ok();
    assert_eq!(
        COUNTER.load(Ordering::SeqCst),
        1,
        "the same guard at controller + method scope is deduped to one execution",
    );
}

#[controller(path = "/g-dedup")]
struct GlobalDedupController;

#[routes]
impl GlobalDedupController {
    #[get("/redeclared")]
    #[use_guards(CountingGuard)]
    async fn redeclared(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [CountingGuard, GlobalDedupController])]
struct GlobalDedupModule;

#[module(imports = [GlobalDedupModule], providers = [GlobalGuards<(CountingGuard,)>])]
struct GlobalGuardRedeclaredPerMethodIsDedupedToOneExecutionApp;

#[tokio::test]
async fn global_guard_redeclared_per_method_is_deduped_to_one_execution() {
    let _gate = GATE.lock().await;
    COUNTER.store(0, Ordering::SeqCst);

    let app = TestApp::builder()
        .module::<GlobalGuardRedeclaredPerMethodIsDedupedToOneExecutionApp>()
        .build()
        .await
        .expect("boots with a global guard");

    app.http()
        .get("/g-dedup/redeclared")
        .send()
        .await
        .assert_status_is_ok();
    assert_eq!(
        COUNTER.load(Ordering::SeqCst),
        1,
        "a guard declared global + method is deduped to one execution",
    );
}

#[controller(path = "/order")]
struct OrderScope;

#[routes]
impl OrderScope {
    // First listed runs first (outermost): authn (401) before authz (403).
    #[get("/x")]
    #[use_guards(ChallengeGuard, DenyGuard)]
    async fn x(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [ChallengeGuard, DenyGuard, OrderScope])]
struct OrderModule;

#[tokio::test]
async fn the_first_listed_guard_runs_before_the_second() {
    let app = TestApp::for_module::<OrderModule>().await.expect("boots");

    app.http()
        .get("/order/x")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// The framework never skips a guard for `#[public]`: a guard reads the posture
// (`cx.reflector().is_public()`) and decides what public means for it.

/// Denies every request *unless* the route is `#[public]`, in which case it
/// admits with no principal — the `AuthnGuard` posture, distilled.
#[injectable]
#[derive(Default)]
struct PublicAwareGuard;

impl Layer for PublicAwareGuard {}

#[async_trait]
impl Guard for PublicAwareGuard {
    async fn check_http(
        &self,
        cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        if cx.reflector().is_public() {
            return Ok(());
        }
        Err(Denial::forbidden("forbidden"))
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for PublicAwareGuard {}

#[controller(path = "/pub")]
struct PublicScope;

#[routes]
impl PublicScope {
    // Handler fn names generate module-global types, so they are unique across
    // the file.
    #[get("/open")]
    #[public]
    async fn pub_open(&self) -> &'static str {
        "ok"
    }

    #[get("/closed")]
    async fn pub_closed(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [PublicAwareGuard, PublicScope])]
struct PublicBypassModule;

#[module(imports = [PublicBypassModule], providers = [GlobalGuards<(PublicAwareGuard,)>])]
struct APublicRouteBypassesAnActiveGlobalGuardApp;

#[tokio::test]
async fn a_public_route_bypasses_an_active_global_guard() {
    let app = TestApp::builder()
        .module::<APublicRouteBypassesAnActiveGlobalGuardApp>()
        .build()
        .await
        .expect("boots with a global guard");

    app.http()
        .get("/pub/open")
        .send()
        .await
        .assert_status_is_ok();

    app.http()
        .get("/pub/closed")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tenant(String);

impl AmbientValue for Tenant {}

/// Reads `x-tenant` (defaulting to `anon`) and attaches it as a `Tenant` for
/// the rest of the request: a route handler reads it through `Ambient<Tenant>`, a
/// resolver from the ambient context.
#[injectable]
#[derive(Default)]
struct TenantGuard;

impl Layer for TenantGuard {}

#[async_trait]
impl Guard for TenantGuard {
    async fn check_http(
        &self,
        cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        let tenant = HttpView::of(cx)?
            .request()
            .header("x-tenant")
            .unwrap_or("anon")
            .to_owned();
        cx.attach(Tenant(tenant));
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for TenantGuard {}

#[controller(path = "/ctx")]
#[use_guards(TenantGuard)]
struct CtxScope;

#[routes]
impl CtxScope {
    #[get("/whoami")]
    async fn whoami(&self, tenant: Ambient<Tenant>) -> String {
        tenant.into_inner().0
    }
}

#[module(providers = [TenantGuard, CtxScope])]
struct CtxModule;

#[tokio::test]
async fn a_guard_attached_context_is_read_back_by_the_handler() {
    let app = TestApp::for_module::<CtxModule>().await.expect("boots");

    let resp = app
        .http()
        .get("/ctx/whoami")
        .header("x-tenant", "acme")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("acme").await;

    let anon = app.http().get("/ctx/whoami").send().await;
    anon.assert_status_is_ok();
    anon.assert_text("anon").await;
}

#[resolver]
struct TenantResolver;

#[operations]
impl TenantResolver {
    #[query]
    #[public]
    async fn tenant(&self) -> GqlResult<String> {
        Ok(nest_rs_core::ambient::<Tenant>().map_or_else(String::new, |tenant| tenant.0.clone()))
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [TenantGuard, TenantResolver],
)]
struct TenantGraphqlModule;

#[module(imports = [TenantGraphqlModule], providers = [GlobalGuards<(TenantGuard,)>])]
struct AGuardAttachedContextOnTheGraphqlRequestIsReadByAResolverApp;

/// The `/graphql` request runs the pool once, in band; what it attached is
/// ambient for every field the request resolves.
#[tokio::test]
async fn a_guard_attached_context_on_the_graphql_request_is_read_by_a_resolver() {
    let app = TestApp::builder()
        .module::<AGuardAttachedContextOnTheGraphqlRequestIsReadByAResolverApp>()
        .build()
        .await
        .expect("a resolver under a pooled guard boots");

    let resp = app
        .http()
        .post("/graphql")
        .header("x-tenant", "acme")
        .body_json(&serde_json::json!({ "query": "{ tenant }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(body.contains(r#""tenant":"acme""#), "{body}");
}

#[controller(path = "/ctx-bare")]
struct CtxUnguarded;

#[routes]
impl CtxUnguarded {
    #[get("/whoami")]
    #[public]
    async fn bare_whoami(&self, tenant: Ambient<Tenant>) -> String {
        tenant.into_inner().0
    }
}

#[module(providers = [CtxUnguarded])]
struct CtxUnguardedModule;

#[tokio::test]
async fn a_missing_context_is_a_bare_500_without_leaking_the_rust_type() {
    let app = TestApp::for_module::<CtxUnguardedModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/ctx-bare/whoami").send().await;
    resp.assert_status(nest_rs_http::StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("Tenant"),
        "the response body must not leak the context type name: {body:?}"
    );
}

/// Attaches the principal: an authentication-phase guard.
#[injectable]
#[derive(Default)]
struct SessionGuard;

impl Layer for SessionGuard {}

#[async_trait]
impl Guard for SessionGuard {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authentication
    }

    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for SessionGuard {}

/// Reads the principal: an authorization-phase guard.
#[injectable]
#[derive(Default)]
struct PolicyGuard;

impl Layer for PolicyGuard {}

#[async_trait]
impl Guard for PolicyGuard {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for PolicyGuard {}

#[controller(path = "/misordered")]
struct MisorderedScope;

#[routes]
impl MisorderedScope {
    #[get("/x")]
    #[use_guards(PolicyGuard, SessionGuard)]
    async fn x(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [SessionGuard, PolicyGuard, MisorderedScope])]
struct MisorderedModule;

#[tokio::test]
async fn a_route_whose_method_scope_lists_authorization_first_fails_the_boot_naming_both() {
    let Err(refused) = TestApp::for_module::<MisorderedModule>().await else {
        panic!("the route would answer 500 on every request instead");
    };
    let chain = format!("{refused:#}");
    assert!(
        chain.contains("PolicyGuard") && chain.contains("SessionGuard"),
        "the refusal names both guards: {chain}",
    );
    assert!(
        chain.contains("GET /misordered/x") && chain.contains("MisorderedScope"),
        "…and the route they are misordered on, its whole path and its controller: {chain}",
    );
}

#[module(providers = [SessionGuard, PolicyGuard])]
struct PhasedGuardsModule;

#[module(imports = [PhasedGuardsModule], providers = [GlobalGuards<(PolicyGuard, SessionGuard)>])]
struct AGlobalPoolListingAuthorizationFirstFailsTheBootOfAnAppServingNoHttpApp;

#[tokio::test]
async fn a_global_pool_listing_authorization_first_fails_the_boot_of_an_app_serving_no_http() {
    let result = HeadlessApp::builder()
        .module::<AGlobalPoolListingAuthorizationFirstFailsTheBootOfAnAppServingNoHttpApp>()
        .build_headless()
        .await;
    let (registry, chain) = refused_wiring(
        result,
        "the app would boot with a pool that answers 500 wherever it runs",
    );
    assert_eq!(registry, "GlobalGuards", "{chain}");
    assert!(
        chain.contains("PolicyGuard") && chain.contains("SessionGuard"),
        "the refusal names both guards: {chain}",
    );
}

static FRESH_SESSION_RUNS: AtomicUsize = AtomicUsize::new(0);

/// An authentication-phase guard that counts its runs, so a replay is seen.
#[injectable]
#[derive(Default)]
struct FreshSessionGuard;

impl Layer for FreshSessionGuard {}

#[async_trait]
impl Guard for FreshSessionGuard {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authentication
    }

    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        FRESH_SESSION_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for FreshSessionGuard {}

#[controller(path = "/replayed")]
struct ReplayedScope;

#[routes]
impl ReplayedScope {
    #[get("/x")]
    #[force_guards(FreshSessionGuard)]
    async fn x(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [FreshSessionGuard, PolicyGuard, ReplayedScope])]
struct ReplayedModule;

#[module(imports = [ReplayedModule], providers = [GlobalGuards<(FreshSessionGuard, PolicyGuard)>])]
struct AForcedAuthenticationReplayAfterTheGlobalPoolBootsAndRunsAgainApp;

#[tokio::test]
async fn a_forced_authentication_replay_after_the_global_pool_boots_and_runs_again() {
    let app = TestApp::builder()
        .module::<AForcedAuthenticationReplayAfterTheGlobalPoolBootsAndRunsAgainApp>()
        .build()
        .await
        .unwrap_or_else(|err| panic!("a replay is no misorder: {err:#}"));

    app.http()
        .get("/replayed/x")
        .send()
        .await
        .assert_status_is_ok();
    assert_eq!(
        FRESH_SESSION_RUNS.load(Ordering::SeqCst),
        2,
        "the global run, then the replay after the authorization guard",
    );
}

/// A guard's `cx.require::<T>()` answers by cause: the principal it declares
/// expecting, absent where a guard attaching it ran and admitted a visitor, is
/// the caller's `401`, on every edge; nothing attaching it refuses the boot.
mod required_principal {
    use nest_rs_core::{Posture, UnitContext, UnitView};
    use nest_rs_graphql::GraphqlView;
    use nest_rs_guards::{GuardPhase, PrincipalClaim};
    use nest_rs_http::EndpointGuards;
    use nest_rs_mcp::{McpError, McpView, mcp, tools};
    use nest_rs_ws::{WsClient, WsModule, gateway, messages};
    use serde_json::json;

    use super::*;

    #[derive(Clone)]
    struct Claims;

    impl AmbientValue for Claims {}

    /// Attaches [`Claims`] for a caller sending `x-user`; admits the rest where
    /// the site takes a visitor, refuses them elsewhere.
    #[injectable]
    #[derive(Default)]
    struct Authn;

    impl Layer for Authn {}

    #[async_trait]
    impl Guard for Authn {
        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            nest_rs_core::__private::set_actor_id("caller");
            if HttpView::of(cx)?.request().header("x-user").is_some() {
                cx.attach(Claims);
                return Ok(());
            }
            match cx.reflector().posture() {
                Posture::Public | Posture::InBand => Ok(()),
                _ => Err(Denial::unauthorized("a credential is required")),
            }
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }

        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }

        fn produced_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<Claims>())
        }
    }

    impl GuardFor<HttpView> for Authn {}

    /// One decision on every edge: the caller's claims, read and required.
    #[injectable]
    #[derive(Default)]
    struct NeedsClaims;

    impl Layer for NeedsClaims {}

    #[async_trait]
    impl Guard for NeedsClaims {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            cx.require::<Claims>()?;
            Ok(())
        }

        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<Claims>())
        }
    }

    impl<V: UnitView> GuardFor<V> for NeedsClaims {}

    #[controller(path = "/required")]
    #[use_guards(Authn)]
    struct RequiredController;

    #[routes]
    impl RequiredController {
        #[get("/open")]
        #[public]
        #[use_guards(NeedsClaims)]
        async fn open(&self) -> &'static str {
            "reached"
        }

        #[get("/gated")]
        #[use_guards(NeedsClaims)]
        async fn gated(&self) -> &'static str {
            "reached"
        }
    }

    #[resolver]
    struct RequiredResolver;

    #[operations]
    impl RequiredResolver {
        #[query]
        #[public]
        #[use_guards(NeedsClaims)]
        async fn required(&self) -> i32 {
            1
        }
    }

    #[module(
        imports = [GraphqlModule],
        providers = [
            Authn,
            NeedsClaims,
            RequiredController,
            RequiredResolver,
            EndpointGuards<GraphqlView, (Authn,)>,
        ],
    )]
    struct RequiredModule;

    async fn app() -> TestApp {
        TestApp::for_module::<RequiredModule>()
            .await
            .expect("boots")
    }

    #[tokio::test]
    async fn a_visitor_a_public_route_admitted_answers_401_never_a_wiring_500() {
        let app = app().await;
        app.http()
            .get("/required/open")
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        app.http()
            .get("/required/open")
            .header("x-user", "ada")
            .send()
            .await
            .assert_status(StatusCode::OK);
    }

    #[tokio::test]
    async fn the_guard_writes_one_check_and_a_gated_route_refuses_at_authentication() {
        let app = app().await;
        app.http()
            .get("/required/gated")
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        app.http()
            .get("/required/gated")
            .header("x-user", "ada")
            .send()
            .await
            .assert_status(StatusCode::OK);
    }

    #[tokio::test]
    async fn a_visitor_in_band_graphql_admitted_answers_unauthenticated() {
        let app = app().await;
        let refused: serde_json::Value = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": "{ required }" }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize();
        assert_eq!(
            refused["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
            "{refused}",
        );
        let served: serde_json::Value = app
            .http()
            .post("/graphql")
            .header("x-user", "ada")
            .body_json(&json!({ "query": "{ required }" }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize();
        assert_eq!(served["data"]["required"], 1, "{served}");
    }

    async fn boot_refusal<M: nest_rs_core::Module + 'static>() -> String {
        match TestApp::for_module::<M>().await {
            Ok(_) => panic!("nothing attaches the principal the guard requires"),
            Err(err) => format!("{err:#}"),
        }
    }

    fn names_the_gap(said: &str) {
        assert!(
            said.contains("NeedsClaims") && said.contains("Claims"),
            "names the guard and the principal: {said}",
        );
    }

    #[controller(path = "/unwired")]
    struct UnwiredRouteController;

    #[routes]
    impl UnwiredRouteController {
        #[get("/")]
        #[use_guards(NeedsClaims)]
        async fn route(&self) -> &'static str {
            "unreachable"
        }
    }

    #[module(providers = [NeedsClaims, UnwiredRouteController])]
    struct UnwiredRouteModule;

    #[tokio::test]
    async fn a_route_nothing_attaches_the_required_principal_on_refuses_the_boot() {
        names_the_gap(&boot_refusal::<UnwiredRouteModule>().await);
    }

    #[controller(path = "/unwired-open")]
    struct UnwiredPublicRouteController;

    #[routes]
    impl UnwiredPublicRouteController {
        #[get("/")]
        #[public]
        #[use_guards(NeedsClaims)]
        async fn route(&self) -> &'static str {
            "unreachable"
        }
    }

    #[module(providers = [NeedsClaims, UnwiredPublicRouteController])]
    struct UnwiredPublicRouteModule;

    #[tokio::test]
    async fn a_public_route_is_refused_it_too_where_only_a_visitor_could_ever_arrive() {
        names_the_gap(&boot_refusal::<UnwiredPublicRouteModule>().await);
    }

    #[resolver]
    struct UnwiredResolver;

    #[operations]
    impl UnwiredResolver {
        #[query]
        #[public]
        #[use_guards(NeedsClaims)]
        async fn unwired(&self) -> i32 {
            1
        }
    }

    #[module(imports = [GraphqlModule], providers = [NeedsClaims, UnwiredResolver])]
    struct UnwiredFieldModule;

    #[tokio::test]
    async fn a_graphql_field_nothing_attaches_the_required_principal_on_refuses_the_boot() {
        names_the_gap(&boot_refusal::<UnwiredFieldModule>().await);
    }

    #[gateway(path = "/ws/unwired")]
    struct UnwiredGateway;

    #[messages]
    impl UnwiredGateway {
        #[subscribe_message("ping")]
        #[use_guards(NeedsClaims)]
        async fn ping(&self, _client: &WsClient) -> String {
            "unreachable".into()
        }

        #[subscribe_message("open")]
        #[public]
        async fn open(&self, _client: &WsClient) -> String {
            "the line opening the connection".into()
        }
    }

    #[module(imports = [WsModule], providers = [NeedsClaims, UnwiredGateway])]
    struct UnwiredMessageModule;

    #[tokio::test]
    async fn a_message_nothing_attaches_the_required_principal_on_refuses_the_boot() {
        names_the_gap(&boot_refusal::<UnwiredMessageModule>().await);
    }

    #[gateway(path = "/ws/wired")]
    #[use_guards(Authn)]
    struct WiredGateway;

    #[messages]
    impl WiredGateway {
        #[subscribe_message("ping")]
        #[use_guards(NeedsClaims)]
        async fn ping(&self, _client: &WsClient) -> String {
            "served".into()
        }
    }

    #[module(imports = [WsModule], providers = [Authn, NeedsClaims, WiredGateway])]
    struct WiredMessageModule;

    #[tokio::test]
    async fn a_message_reads_the_principal_its_upgrade_attached() {
        TestApp::for_module::<WiredMessageModule>()
            .await
            .expect("the upgrade's guard attaches it");
    }

    #[mcp(path = "/mcp/unwired")]
    #[derive(Clone, Default)]
    struct UnwiredTool;

    #[tools]
    impl UnwiredTool {
        /// Answer.
        #[tool]
        #[use_guards(NeedsClaims)]
        async fn answer(&self) -> Result<String, McpError> {
            Ok("unreachable".to_owned())
        }
    }

    #[module(providers = [NeedsClaims, UnwiredTool])]
    struct UnwiredOperationModule;

    #[tokio::test]
    async fn an_mcp_operation_nothing_attaches_the_required_principal_on_refuses_the_boot() {
        names_the_gap(&boot_refusal::<UnwiredOperationModule>().await);
    }

    #[module(providers = [Authn, NeedsClaims, UnwiredTool, EndpointGuards<McpView, (Authn,)>])]
    struct WiredOperationModule;

    #[tokio::test]
    async fn an_mcp_operation_reads_the_principal_its_endpoint_guards_attached() {
        TestApp::for_module::<WiredOperationModule>()
            .await
            .expect("the endpoint's guard attaches it");
    }
}
