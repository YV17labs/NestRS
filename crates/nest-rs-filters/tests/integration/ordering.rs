//! `.filter()` composition (HTTP-T3). A `Filter` maps a handler `Err` to a
//! response; when several are stacked the **innermost** (closest to the handler)
//! maps first and turns the error into `Ok`, so outer filters never see it. A
//! successful handler passes through every filter unmapped. Every mapped
//! response carries the `MappedError` rollback tag.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_filters::{Filter, FilterExt, RequestSnapshot};
use poem::http::StatusCode;
use poem::{Endpoint, IntoResponse, Request, Response, endpoint::make};

type Trace = Arc<Mutex<Vec<String>>>;

fn trace() -> Trace {
    Arc::new(Mutex::new(Vec::new()))
}

/// Maps any error to `status`, recording that it ran.
struct LogFilter {
    name: &'static str,
    status: StatusCode,
    trace: Trace,
}

impl Layer for LogFilter {}

#[async_trait]
impl Filter for LogFilter {
    async fn filter(&self, _req: &RequestSnapshot, _error: poem::Error) -> Response {
        self.trace
            .lock()
            .expect("trace lock")
            .push(self.name.into());
        self.status.into_response()
    }
}

/// A handler that always fails with a 500.
fn failing() -> impl Endpoint<Output = Response> {
    make(|_req: Request| async {
        Err::<Response, _>(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    })
}

/// A handler that always succeeds.
fn succeeding() -> impl Endpoint<Output = Response> {
    make(|_req: Request| async { Ok::<_, poem::Error>("ok".into_response()) })
}

#[tokio::test]
async fn the_innermost_filter_maps_and_outer_filters_never_see_the_error() {
    let trace = trace();
    let inner = LogFilter {
        name: "inner",
        status: StatusCode::IM_A_TEAPOT,
        trace: trace.clone(),
    };
    let outer = LogFilter {
        name: "outer",
        status: StatusCode::BAD_GATEWAY,
        trace: trace.clone(),
    };

    // `.filter(inner).filter(outer)` → inner is closest to the handler.
    let endpoint = failing().filter(inner).filter(outer);
    let resp = endpoint
        .call(Request::default())
        .await
        .expect("the error is mapped, not propagated");

    // Inner mapped the error to a teapot and turned it into `Ok`, so outer's
    // filter body never ran and its status never applied.
    assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(
        trace.lock().expect("trace lock").clone(),
        ["inner"],
        "only the innermost filter maps the handler's error",
    );
    assert!(
        resp.extensions()
            .get::<nest_rs_core::MappedError>()
            .is_some(),
        "a mapped error response carries the rollback tag",
    );
}

#[tokio::test]
async fn a_successful_handler_passes_through_every_filter_unmapped() {
    let trace = trace();
    let inner = LogFilter {
        name: "inner",
        status: StatusCode::IM_A_TEAPOT,
        trace: trace.clone(),
    };
    let outer = LogFilter {
        name: "outer",
        status: StatusCode::BAD_GATEWAY,
        trace: trace.clone(),
    };

    let endpoint = succeeding().filter(inner).filter(outer);
    let resp = endpoint
        .call(Request::default())
        .await
        .expect("success flows through");

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        trace.lock().expect("trace lock").is_empty(),
        "no filter runs when the handler succeeds",
    );
    assert!(
        resp.extensions()
            .get::<nest_rs_core::MappedError>()
            .is_none(),
        "an unmapped success is never tagged for rollback",
    );
}
