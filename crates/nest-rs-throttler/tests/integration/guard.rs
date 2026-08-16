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

use nest_rs_core::module;
use nest_rs_guards::guard;
use nest_rs_http::{controller, routes};
use nest_rs_testing::TestApp;
use nest_rs_throttler::{Throttle, ThrottlerConfig, ThrottlerGuard, ThrottlerModule};
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
    // bucket filled. `CLAUDE.md` ranks a rate-limit denial with the security
    // events an incident queries, and a composite key it does not carry is a
    // denial nobody can attribute to a route or a caller.
    let event = logs.expect_one("nest_rs::throttler", "rate limit exceeded");
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("key")
            .is_some_and(|k| k.contains("/rated/strict")),
        "the event names the route half of the composite key, got {:?}",
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
