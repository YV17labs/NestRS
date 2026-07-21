//! `.interceptor()` composition order (HTTP-T2). Two properties a single-
//! interceptor unit test can't show:
//!
//! 1. Interceptors **nest outermost-first**: `ep.interceptor(a).interceptor(b)`
//!    runs `b` around `a` around the handler, so `b` sees the request first and
//!    the response last.
//! 2. An interceptor that returns **without calling `next.run`** short-circuits
//!    — every inner interceptor and the handler are skipped.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_interceptors::{Interceptor, InterceptorExt, Next};
use poem::http::StatusCode;
use poem::{Endpoint, IntoResponse, Request, Response, Result, endpoint::make_sync};

/// A shared, ordered trace of who ran, in the order they ran.
type Trace = Arc<Mutex<Vec<String>>>;

fn trace() -> Trace {
    Arc::new(Mutex::new(Vec::new()))
}

fn record(trace: &Trace, entry: impl Into<String>) {
    trace.lock().expect("trace lock").push(entry.into());
}

/// Records `enter:<name>` before delegating and `exit:<name>` after — so the
/// trace shows the full nesting, not just entry order.
struct LogInterceptor {
    name: &'static str,
    trace: Trace,
}

impl Layer for LogInterceptor {}

#[async_trait]
impl Interceptor for LogInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        record(&self.trace, format!("enter:{}", self.name));
        let resp = next.run(req).await;
        record(&self.trace, format!("exit:{}", self.name));
        resp
    }
}

/// Never calls `next.run` — answers directly, so nothing inner runs.
struct ShortCircuit {
    trace: Trace,
}

impl Layer for ShortCircuit {}

#[async_trait]
impl Interceptor for ShortCircuit {
    async fn intercept(&self, _req: Request, _next: Next<'_>) -> Result<Response> {
        record(&self.trace, "short-circuit");
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

/// A handler that records that it ran and returns a body.
fn handler(trace: Trace) -> impl Endpoint<Output = Response> {
    make_sync(move |_req: Request| {
        record(&trace, "handler");
        "ok".into_response()
    })
}

#[tokio::test]
async fn interceptors_nest_outermost_first() {
    let trace = trace();
    let inner = LogInterceptor {
        name: "inner",
        trace: trace.clone(),
    };
    let outer = LogInterceptor {
        name: "outer",
        trace: trace.clone(),
    };

    // `.interceptor(inner).interceptor(outer)` → outer wraps inner wraps handler.
    let endpoint = handler(trace.clone()).interceptor(inner).interceptor(outer);
    let resp = endpoint
        .call(Request::default())
        .await
        .expect("chain runs to the handler");
    assert_eq!(resp.status(), StatusCode::OK);

    let order = trace.lock().expect("trace lock").clone();
    assert_eq!(
        order,
        [
            "enter:outer",
            "enter:inner",
            "handler",
            "exit:inner",
            "exit:outer",
        ],
        "outer must see the request first and the response last",
    );
}

#[tokio::test]
async fn a_short_circuit_skips_every_inner_link_and_the_handler() {
    let trace = trace();
    let inner = LogInterceptor {
        name: "inner",
        trace: trace.clone(),
    };
    let short = ShortCircuit {
        trace: trace.clone(),
    };

    // `short` is outermost; it answers without delegating, so neither `inner`
    // nor the handler runs.
    let endpoint = handler(trace.clone()).interceptor(inner).interceptor(short);
    let resp = endpoint
        .call(Request::default())
        .await
        .expect("short-circuit still produces a response");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let order = trace.lock().expect("trace lock").clone();
    assert_eq!(
        order,
        ["short-circuit"],
        "nothing inner runs once an interceptor short-circuits: {order:?}",
    );
}
