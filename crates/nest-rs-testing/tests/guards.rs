//! Guard effectiveness across the three Layer-System scopes — **handler**,
//! **controller**, and **global** (`use_guards_global`) — plus multi-guard
//! ordering and unguarded (public) routes, driven end-to-end through the HTTP
//! harness. One `#[test]` per scenario; the fixtures are tiny inline
//! controllers, no product entities and no database.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{HandlerMetadata, Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
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
//
// Scope is a *declaration* concern: a guard named at several scopes belongs to
// one pool, is deduplicated by `TypeId`, and executes **exactly once** per
// request. A *denying* guard cannot prove "once" — a 403 looks identical whether
// the guard ran one time or five. So these two tests use a *counting* guard that
// increments a process-global counter and then admits: the response is always
// `200` and the counter is the assertion surface, making the dedup real coverage
// rather than a tautology. (The full seven-combination sweep lives in
// `layer_pool.rs`; these keep the guards-file coverage honest.)

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

    // Controller scope only: one declaration, one execution.
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

    // Controller *and* method scope: two declarations of the same `TypeId`,
    // deduped to a single execution. A broken dedup would count 2 here.
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

    // Global declares `CountingGuard`; the method re-declares it. The per-route
    // shaper composes global + method and dedups by `TypeId` (broadest scope
    // wins), so the guard runs once — a broken dedup would count 2.
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

// --- #[public] bypasses an active global guard --------------------------------
//
// A `#[public]` route attaches a `Public` marker to its route metadata. The
// framework never skips a guard on its behalf — a guard *reads* the marker (via
// `Reflector::is_public`) and decides what public means for it. `AuthGuard` uses
// exactly this to let an anonymous request through a public route; here a
// minimal guard distills that posture: admit when public, deny otherwise. The
// global guard runs post-routing in the `RouteShaper`, so the marker is already
// attached by the time it reads it.

/// Denies every request *unless* the route is `#[public]`, in which case it
/// admits with no principal — the `AuthGuard` posture, distilled.
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

#[controller(path = "/pub")]
struct PublicScope;

#[routes]
impl PublicScope {
    // Handler fn names generate module-global types, so keep them unique across
    // the file (`pub_*`); the URL paths stay `/open` and `/closed`.
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

    // The global guard runs post-routing, reads `#[public]`, and admits — the
    // public route answers 200 without a principal.
    app.http().get("/pub/open").send().await.assert_status_is_ok();

    // The sibling route carries no `#[public]`, so the same global guard denies.
    app.http()
        .get("/pub/closed")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

// --- Ctx<T> round-trip: guard attaches, handler reads back --------------------
//
// A guard may attach request-scoped context (the authenticated principal is the
// canonical case) by inserting it into the request extensions; a handler reads
// it back with `Ctx<T>`. This pins that hand-off end-to-end.

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

#[controller(path = "/ctx")]
#[use_guards(AttachPrincipalGuard)]
struct CtxScope;

#[routes]
impl CtxScope {
    // `Ctx<Principal>` extracts the value the guard attached; its presence is
    // what arms the round-trip — remove the guard and this rejects with 500.
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

    // The guard reads `x-user` and attaches it; the handler echoes what `Ctx`
    // hands back — proving the value survived guard → handler.
    let resp = app
        .http()
        .get("/ctx/whoami")
        .header("x-user", "alice")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("alice").await;

    // Default branch: no header ⇒ the guard attaches `anon`, still round-tripped.
    let anon = app.http().get("/ctx/whoami").send().await;
    anon.assert_status_is_ok();
    anon.assert_text("anon").await;
}
