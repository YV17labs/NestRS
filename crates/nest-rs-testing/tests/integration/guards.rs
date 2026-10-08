//! Guard effectiveness across the three scopes — **handler**, **controller**
//! and **global** — plus multi-guard ordering and `#[public]` routes.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::HandlerMetadata;
use nest_rs_http::{Ctx, Reflector, async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::Request;
use poem::http::StatusCode;
use tokio::sync::Mutex;

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

impl HttpGuard for DenyGuard {}

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

impl HttpGuard for ChallengeGuard {}

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
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl HttpGuard for CountingGuard {}

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

#[tokio::test]
async fn global_guard_redeclared_per_method_is_deduped_to_one_execution() {
    let _gate = GATE.lock().await;
    COUNTER.store(0, Ordering::SeqCst);

    let app = TestApp::builder()
        .module::<GlobalDedupModule>()
        .use_guards_global([guard::<CountingGuard>()])
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

// The framework never skips a guard for `#[public]`: a guard reads the marker
// (`Reflector::is_public`) and decides what public means for it.

/// Denies every request *unless* the route is `#[public]`, in which case it
/// admits with no principal — the `AuthnGuard` posture, distilled.
#[injectable]
#[derive(Default)]
struct PublicAwareGuard;

impl Layer for PublicAwareGuard {}

#[async_trait]
impl Guard for PublicAwareGuard {
    async fn check_http(&self, req: &mut Request) -> std::result::Result<(), Denial> {
        if Reflector::new(req).is_public() {
            return Ok(());
        }
        Err(Denial::forbidden("forbidden"))
    }
}

impl HttpGuard for PublicAwareGuard {}

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

#[tokio::test]
async fn a_public_route_bypasses_an_active_global_guard() {
    let app = TestApp::builder()
        .module::<PublicBypassModule>()
        .use_guards_global([guard::<PublicAwareGuard>()])
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
struct Principal(String);

/// Reads `x-user` (defaulting to `anon`) and attaches it as a `Principal` for
/// the handler to read back through `Ctx<Principal>`.
#[injectable]
#[derive(Default)]
struct AttachPrincipalGuard;

impl Layer for AttachPrincipalGuard {}

#[async_trait]
impl Guard for AttachPrincipalGuard {
    async fn check_http(&self, req: &mut Request) -> std::result::Result<(), Denial> {
        let who = req
            .headers()
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("anon")
            .to_owned();
        req.extensions_mut().insert(Principal(who));
        Ok(())
    }
}

impl HttpGuard for AttachPrincipalGuard {}

#[controller(path = "/ctx")]
#[use_guards(AttachPrincipalGuard)]
struct CtxScope;

#[routes]
impl CtxScope {
    #[get("/whoami")]
    async fn whoami(&self, principal: Ctx<Principal>) -> String {
        principal.into_inner().0
    }
}

#[module(providers = [AttachPrincipalGuard, CtxScope])]
struct CtxModule;

#[tokio::test]
async fn a_guard_attached_context_is_read_back_by_the_handler() {
    let app = TestApp::for_module::<CtxModule>().await.expect("boots");

    let resp = app
        .http()
        .get("/ctx/whoami")
        .header("x-user", "alice")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("alice").await;

    let anon = app.http().get("/ctx/whoami").send().await;
    anon.assert_status_is_ok();
    anon.assert_text("anon").await;
}

#[controller(path = "/ctx-bare")]
struct CtxUnguarded;

#[routes]
impl CtxUnguarded {
    #[get("/whoami")]
    async fn bare_whoami(&self, principal: Ctx<Principal>) -> String {
        principal.into_inner().0
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
    resp.assert_status(poem::http::StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("Principal"),
        "the response body must not leak the context type name: {body:?}"
    );
}
