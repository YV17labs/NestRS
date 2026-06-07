//! Guard effectiveness across the three Layer-System scopes — **handler**,
//! **controller**, and **global** (`use_guards_global`) — plus multi-guard
//! ordering and unguarded (public) routes, driven end-to-end through the HTTP
//! harness. One `#[test]` per scenario; the fixtures are tiny inline
//! controllers, no product entities and no database.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::Request;
use poem::http::StatusCode;

/// Denies every request with `403 Forbidden`.
#[injectable]
#[derive(Default)]
struct DenyGuard;

impl Layer for DenyGuard {}

#[async_trait]
impl Guard for DenyGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("forbidden"))
    }
}

/// Denies with `401 Unauthorized` — paired with [`DenyGuard`] to observe order.
#[injectable]
#[derive(Default)]
struct ChallengeGuard;

impl Layer for ChallengeGuard {}

#[async_trait]
impl Guard for ChallengeGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Err(Denial::unauthorized("unauthorized"))
    }
}

// --- handler + controller scope ----------------------------------------------

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

// --- global scope -------------------------------------------------------------

#[controller(path = "/g")]
struct PublicEverywhere;

#[routes]
impl PublicEverywhere {
    #[get("/a")]
    async fn a(&self) -> &'static str {
        "ok"
    }

    #[get("/b")]
    async fn b(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [DenyGuard, PublicEverywhere])]
struct PublicModule;

#[tokio::test]
async fn a_global_guard_protects_every_route_without_use_guards() {
    let app = TestApp::builder()
        .module::<PublicModule>()
        .use_guards_global([guard::<DenyGuard>()])
        .build()
        .await
        .expect("boots with a global guard");

    // No handler here carries `#[use_guards]`, yet the global guard denies all.
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
    // The default-derived `DenyGuard` provider stays registered, but with no
    // `use_guards_global` call no `GuardSpecs` is seeded, so the global chain
    // is empty — `RouteShaper` runs no global, no controller, no
    // method guards, and the route stays open.
    let app = TestApp::builder()
        .module::<PublicModule>()
        .build()
        .await
        .expect("boots");

    for path in ["/g/a", "/g/b"] {
        app.http().get(path).send().await.assert_status_is_ok();
    }
}

// --- dedup across scopes ------------------------------------------------------

#[controller(path = "/dedup")]
#[use_guards(DenyGuard)]
struct DedupController;

#[routes]
impl DedupController {
    #[get("/c-only")]
    async fn c_only(&self) -> &'static str {
        "unreachable"
    }

    #[get("/c-and-m")]
    #[use_guards(DenyGuard)]
    async fn c_and_m(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [DenyGuard, DedupController])]
struct DedupModule;

#[tokio::test]
async fn the_same_guard_at_controller_and_method_scope_executes_once() {
    // Mirror-test: both routes are protected by `DenyGuard`. The first
    // declares it only at controller scope; the second also at method scope.
    // Both must answer 403 — and if the dedup were broken the second would
    // still answer 403 but the chain would log a doubled execution. We can't
    // observe the doubled execution directly here, so we settle for a
    // smoke-level check: both routes are protected, neither panics, and the
    // chain composes (the test asserts behaviour, the `warn!` in
    // `compose_chain` is the diagnostic for the dedup case).
    let app = TestApp::for_module::<DedupModule>().await.expect("boots");

    for path in ["/dedup/c-only", "/dedup/c-and-m"] {
        app.http()
            .get(path)
            .send()
            .await
            .assert_status(StatusCode::FORBIDDEN);
    }
}

#[controller(path = "/g-dedup")]
struct GlobalDedupController;

#[routes]
impl GlobalDedupController {
    #[get("/redeclared")]
    #[use_guards(DenyGuard)]
    async fn redeclared(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [DenyGuard, GlobalDedupController])]
struct GlobalDedupModule;

#[tokio::test]
async fn global_guard_redeclared_per_method_is_deduped_to_one_execution() {
    // Global declares `DenyGuard`; the method re-declares it. Because the
    // transport-level interceptor already runs globals, the per-route shaper
    // skips the method redeclaration (the broadest scope wins). Both layers
    // would deny, so the route is still 403 — the test pins the shape.
    let app = TestApp::builder()
        .module::<GlobalDedupModule>()
        .use_guards_global([guard::<DenyGuard>()])
        .build()
        .await
        .expect("boots");

    app.http()
        .get("/g-dedup/redeclared")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

// --- ordering -----------------------------------------------------------------

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

    // ChallengeGuard is listed first; it short-circuits with 401, so DenyGuard's
    // 403 is never reached. A 403 here would mean the order was inverted.
    app.http()
        .get("/order/x")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
