//! Boot-time fail-secure contract of the layer pool, mirroring the access
//! graph's posture: a wiring error is a **boot error**, never a runtime
//! surprise.
//!
//! Two checks are pinned here:
//!
//! - a global layer spec whose provider was never registered fails boot
//!   (it would otherwise resolve to `None` and silently drop — for a guard
//!   that means every route quietly loses its fail-secure net);
//! - an imperative `HttpTransport::mount(...)` endpoint — opaque to the
//!   shaper — fails boot when global guards are active, unless the app
//!   explicitly opts down to a warn with `fail_secure_strict(false)`.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{HttpTransport, async_trait, controller, routes};
use nest_rs_interceptors::{Interceptor, Next, interceptor};
use nest_rs_testing::TestApp;
use poem::{Request, Response};

/// Registered as a provider — the resolvable control case.
#[injectable]
#[derive(Default)]
struct WiredGuard;

impl Layer for WiredGuard {}

#[async_trait]
impl Guard for WiredGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostGuard;

impl Layer for GhostGuard {}

#[async_trait]
impl Guard for GhostGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostInterceptor;

impl Layer for GhostInterceptor {}

#[async_trait]
impl Interceptor for GhostInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> poem::Result<Response> {
        next.run(req).await
    }
}

#[module(providers = [WiredGuard])]
struct GuardOnlyModule;

/// `TestApp` is not `Debug`, so unwrap the error arm by hand.
fn boot_error(result: anyhow::Result<TestApp>, expectation: &str) -> anyhow::Error {
    match result {
        Ok(_) => panic!("{expectation}"),
        Err(err) => err,
    }
}

#[tokio::test]
async fn an_unresolvable_global_guard_fails_boot() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_guards_global([guard::<GhostGuard>()])
        .build()
        .await;
    let err = boot_error(
        result,
        "a global guard with no provider must fail boot, not silently drop",
    );
    assert!(
        err.to_string().contains("GhostGuard"),
        "the error names the unresolvable guard: {err}",
    );
}

#[tokio::test]
async fn an_unresolvable_global_interceptor_fails_boot() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_interceptors_global([interceptor::<GhostInterceptor>()])
        .build()
        .await;
    let err = boot_error(
        result,
        "a global interceptor with no provider must fail boot",
    );
    assert!(
        err.to_string().contains("GhostInterceptor"),
        "the error names the unresolvable interceptor: {err}",
    );
}

#[tokio::test]
async fn an_imperative_mount_under_global_guards_fails_boot() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_guards_global([guard::<WiredGuard>()])
        .http(HttpTransport::new().mount("/raw", |_c| poem::endpoint::make_sync(|_req| "open")))
        .build()
        .await;
    let err = boot_error(
        result,
        "an unshapable endpoint under global guards must fail boot in strict mode",
    );
    assert!(
        err.to_string().contains("/raw"),
        "the error names the offending mount path: {err}",
    );
}

#[tokio::test]
async fn fail_secure_strict_off_downgrades_the_mount_violation_to_a_warn() {
    let app = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_guards_global([guard::<WiredGuard>()])
        .http(
            HttpTransport::new()
                .fail_secure_strict(false)
                .mount("/raw", |_c| poem::endpoint::make_sync(|_req| "open")),
        )
        .build()
        .await
        .expect("opting out of strict mode boots with a warn");
    app.http().get("/raw").send().await.assert_status_is_ok();
}

#[tokio::test]
async fn an_imperative_mount_without_global_guards_boots() {
    // No global guards ⇒ nothing for the mount to bypass; strict mode has
    // nothing to enforce.
    let app = TestApp::builder()
        .module::<GuardOnlyModule>()
        .http(HttpTransport::new().mount("/raw", |_c| poem::endpoint::make_sync(|_req| "open")))
        .build()
        .await
        .expect("boots");
    app.http().get("/raw").send().await.assert_status_is_ok();
}

/// A controller route with no guard and no `#[public]` marker — an *implicit*
/// access decision. With no global guard pool to cover it, the transport warns
/// at boot (`access_is_implicit`) but does **not** fail: the warning nudges the
/// developer to make the decision explicit; it never blocks an honest build.
#[controller(path = "/")]
struct OpenController;

#[routes]
impl OpenController {
    #[post("/thing")]
    async fn create(&self) -> String {
        "made".into()
    }
}

#[module(providers = [OpenController])]
struct OpenModule;

#[tokio::test]
async fn an_unguarded_non_public_route_warns_but_boots_without_a_global_pool() {
    let app = TestApp::for_module::<OpenModule>()
        .await
        .expect("an implicit access decision is a warning, not a boot failure");
    // The route is served — the posture check observes, it does not gate.
    app.http().post("/thing").send().await.assert_status_is_ok();
}

/// Two controllers claiming the same prefix. Each `nest`s under it, so the
/// prefix is an exclusive namespace — poem would panic deep in route assembly
/// ("duplicate path: /dup/*--poem-rest"). The transport catches it at boot
/// instead, the same posture as every other wiring error.
#[controller(path = "/dup")]
struct FirstDupController;

#[routes]
impl FirstDupController {
    #[post("/")]
    #[public]
    async fn first(&self) -> String {
        "first".into()
    }
}

#[controller(path = "/dup")]
struct SecondDupController;

#[routes]
impl SecondDupController {
    #[post("/")]
    #[public]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[module(providers = [FirstDupController, SecondDupController])]
struct DuplicatePrefixModule;

#[tokio::test]
async fn two_controllers_sharing_a_prefix_fail_boot_naming_both() {
    let result = TestApp::builder()
        .module::<DuplicatePrefixModule>()
        .build()
        .await;
    let err = boot_error(
        result,
        "two controllers on one prefix must fail boot, not panic inside poem",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("/dup")
            && msg.contains("FirstDupController")
            && msg.contains("SecondDupController"),
        "the error names the shared prefix and both controllers: {err}",
    );
}
