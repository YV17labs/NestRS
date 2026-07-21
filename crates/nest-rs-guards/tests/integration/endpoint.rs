//! `GuardEndpoint`/`GuardExt` — a [`Guard`]'s `check_http` gate in front of a
//! poem endpoint, and the [`Denial`] → HTTP response mapping. The endpoint is
//! driven by calling `.call(..)` directly (no live socket).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use nest_rs_core::Layer;
use nest_rs_guards::{Denial, Guard, GuardExt};
use nest_rs_http::async_trait;
use poem::{Endpoint, IntoResponse, Request, endpoint::make_sync};

/// A guard whose decision is fixed at construction: `None` allows, `Some(d)`
/// denies with `d`. Increments a shared counter each time it runs, so a test
/// can prove a chained guard executed (or was short-circuited).
struct DecisionGuard {
    denial: Option<Denial>,
    ran: Arc<AtomicU32>,
}

impl DecisionGuard {
    fn allow(ran: Arc<AtomicU32>) -> Self {
        Self { denial: None, ran }
    }
    fn deny(denial: Denial, ran: Arc<AtomicU32>) -> Self {
        Self {
            denial: Some(denial),
            ran,
        }
    }
}

impl Layer for DecisionGuard {}

#[async_trait]
impl Guard for DecisionGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        match &self.denial {
            None => Ok(()),
            Some(d) => Err(d.clone()),
        }
    }
}

async fn call_guarded(guard: DecisionGuard) -> poem::Response {
    let ep = make_sync(|_| "ok".into_response()).guard(Arc::new(guard));
    ep.call(Request::builder().finish())
        .await
        .expect("GuardEndpoint maps a denial to Ok(Response), never Err")
}

#[tokio::test]
async fn an_allowing_guard_lets_the_request_reach_the_handler() {
    let resp = call_guarded(DecisionGuard::allow(Arc::new(AtomicU32::new(0)))).await;
    assert_eq!(resp.status(), poem::http::StatusCode::OK);
}

#[tokio::test]
async fn an_unauthorized_denial_renders_401() {
    let resp = call_guarded(DecisionGuard::deny(
        Denial::unauthorized("no token"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), poem::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_forbidden_denial_renders_403() {
    let resp = call_guarded(DecisionGuard::deny(
        Denial::forbidden("not your row"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), poem::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_rate_limit_denial_renders_429_with_retry_after() {
    let resp = call_guarded(DecisionGuard::deny(
        Denial::rate_limited(42, "slow down"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), poem::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("42"),
        "a rate-limit denial must carry Retry-After",
    );
}

#[tokio::test]
async fn a_denial_short_circuits_before_the_handler_runs() {
    // The handler increments a marker; a denying guard in front must keep it 0.
    let handler_ran = Arc::new(AtomicU32::new(0));
    let marker = handler_ran.clone();
    let ep = make_sync(move |_| {
        marker.fetch_add(1, Ordering::SeqCst);
        "ok".into_response()
    })
    .guard(Arc::new(DecisionGuard::deny(
        Denial::forbidden("nope"),
        Arc::new(AtomicU32::new(0)),
    )));
    let resp = ep
        .call(Request::builder().finish())
        .await
        .expect("denial is Ok(Response)");
    assert_eq!(resp.status(), poem::http::StatusCode::FORBIDDEN);
    assert_eq!(
        handler_ran.load(Ordering::SeqCst),
        0,
        "the handler must not run once a guard denies",
    );
}

#[tokio::test]
async fn chained_guards_all_run_when_each_allows() {
    // `.guard(a).guard(b)` — the outer wraps the inner; both `check_http` run
    // before the handler when neither denies.
    let outer_ran = Arc::new(AtomicU32::new(0));
    let inner_ran = Arc::new(AtomicU32::new(0));
    let ep = make_sync(|_| "ok".into_response())
        .guard(Arc::new(DecisionGuard::allow(inner_ran.clone())))
        .guard(Arc::new(DecisionGuard::allow(outer_ran.clone())));
    let resp = ep
        .call(Request::builder().finish())
        .await
        .expect("both allow");
    assert_eq!(resp.status(), poem::http::StatusCode::OK);
    assert_eq!(outer_ran.load(Ordering::SeqCst), 1, "outer guard ran");
    assert_eq!(inner_ran.load(Ordering::SeqCst), 1, "inner guard ran");
}

#[tokio::test]
async fn an_outer_denial_short_circuits_the_inner_guard() {
    // The outer guard denies, so the inner guard must never run.
    let inner_ran = Arc::new(AtomicU32::new(0));
    let ep = make_sync(|_| "ok".into_response())
        .guard(Arc::new(DecisionGuard::allow(inner_ran.clone())))
        .guard(Arc::new(DecisionGuard::deny(
            Denial::unauthorized("stop"),
            Arc::new(AtomicU32::new(0)),
        )));
    let resp = ep
        .call(Request::builder().finish())
        .await
        .expect("denial is Ok(Response)");
    assert_eq!(resp.status(), poem::http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        inner_ran.load(Ordering::SeqCst),
        0,
        "the inner guard must not run once the outer denies",
    );
}
