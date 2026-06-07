//! Guard effectiveness across the three binding scopes — **handler**,
//! **controller**, and **global** (imperative `HttpTransport::guard`) — plus
//! multi-guard ordering and unguarded (public) routes, driven end-to-end
//! through the HTTP harness. One `#[test]` per scenario; the fixtures are tiny
//! inline controllers, no product entities and no database.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{HttpGuard, HttpTransport, async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Request, Response};

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

#[async_trait]
impl HttpGuard for DenyGuard {
    async fn check(&self, _req: &mut Request) -> std::result::Result<(), Response> {
        Err(Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body("forbidden"))
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

#[async_trait]
impl HttpGuard for ChallengeGuard {
    async fn check(&self, _req: &mut Request) -> std::result::Result<(), Response> {
        Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body("unauthorized"))
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

#[module(providers = [PublicEverywhere])]
struct PublicModule;

#[tokio::test]
async fn a_global_guard_protects_every_route_without_use_guards() {
    let app = TestApp::builder()
        .module::<PublicModule>()
        .http(HttpTransport::new().guard(DenyGuard))
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
    let app = TestApp::for_module::<PublicModule>().await.expect("boots");

    for path in ["/g/a", "/g/b"] {
        app.http().get(path).send().await.assert_status_is_ok();
    }
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
