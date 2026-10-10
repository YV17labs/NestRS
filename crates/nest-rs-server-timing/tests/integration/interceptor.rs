//! The documented import, booted, and the header a caller reads back: the
//! interceptor is mounted by the import, so only a boot can see it.

use nest_rs_core::{Discovery, Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::__private::HttpEndpointWrap;
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::poem::{Request, Response};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_interceptors::{Interceptor, Next, interceptor};
use nest_rs_server_timing::ServerTimingModule;
use nest_rs_testing::TestApp;

use crate::{BareModule, in_profile, probe_timing, server_timing};

#[tokio::test]
async fn the_documented_import_puts_the_header_on_a_response() {
    let timing = probe_timing::<BareModule>()
        .await
        .expect("the import is what attaches the header");

    // W3C Server-Timing §3: a comma-separated list of metrics, each a name with
    // optional `;dur=` and `;desc=` parameters.
    assert!(
        timing.contains("db;dur="),
        "the sub-step a handler recorded reaches the wire: {timing}",
    );
}

/// Answers each status twice: as the handler's `Err`, and as a response it
/// built itself.
#[controller(path = "/status")]
struct StatusController;

#[routes]
impl StatusController {
    #[get("/err/{code}")]
    #[public]
    async fn err(
        &self,
        code: nest_rs_http::poem::web::Path<u16>,
    ) -> nest_rs_http::poem::Result<String> {
        Err(nest_rs_http::poem::Error::from_status(status(code.0)))
    }

    #[get("/ok/{code}")]
    #[public]
    async fn ok(&self, code: nest_rs_http::poem::web::Path<u16>) -> Response {
        Response::builder().status(status(code.0)).finish()
    }
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).expect("a status the test names")
}

#[module(imports = [ServerTimingModule], providers = [StatusController])]
struct StatusModule;

/// The `server-timing` header `/status/{shape}/{code}` answers with, once its
/// status is checked.
async fn status_timing(app: &TestApp, shape: &str, code: u16) -> Option<String> {
    let resp = app
        .http()
        .get(format!("/status/{shape}/{code}"))
        .send()
        .await;
    resp.assert_status(status(code));
    server_timing(&resp)
}

/// W3C Server Timing §4: a server may give its metrics to authenticated callers
/// "and nothing at all to all others" — so no refusal of a credential or of a
/// rate is a timing oracle, whatever the profile.
#[tokio::test]
async fn an_authentication_or_rate_refusal_carries_no_timing() {
    let app = TestApp::for_module::<StatusModule>().await.expect("boots");

    for code in [401, 403, 407, 429] {
        for shape in ["err", "ok"] {
            assert_eq!(
                status_timing(&app, shape, code).await,
                None,
                "a {code} answered as `{shape}` carries no timing",
            );
        }
    }

    for code in [400, 404, 500, 503] {
        for shape in ["err", "ok"] {
            assert!(
                status_timing(&app, shape, code).await.is_some(),
                "any other error still says where its time went ({code}, `{shape}`)",
            );
        }
    }
}

/// The header both gates below admit on; any other request is refused `401`.
const ADMIT: &str = "x-admit";

#[injectable]
#[derive(Default)]
struct AdmitGuard;

impl Layer for AdmitGuard {}

#[async_trait]
impl Guard for AdmitGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        if req.headers().contains_key(ADMIT) {
            Ok(())
        } else {
            Err(Denial::unauthorized("no"))
        }
    }
}

impl HttpGuard for AdmitGuard {}

#[controller(path = "/private")]
struct PrivateController;

#[routes]
impl PrivateController {
    #[get("/")]
    async fn index(&self) -> String {
        "secret".into()
    }
}

#[module(imports = [ServerTimingModule], providers = [PrivateController, AdmitGuard])]
struct GuardedModule;

/// The `server-timing` header `/private` answers with, once its status is
/// checked: `200` when the request bears [`ADMIT`], `401` otherwise.
async fn private_timing(app: &TestApp, admitted: bool) -> Option<String> {
    let mut req = app.http().get("/private");
    if admitted {
        req = req.header(ADMIT, "1");
    }
    let resp = req.send().await;
    resp.assert_status(if admitted {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    });
    server_timing(&resp)
}

#[tokio::test]
async fn a_guard_denial_carries_no_timing() {
    let app = TestApp::builder()
        .module::<GuardedModule>()
        .use_guards_global([guard::<AdmitGuard>()])
        .build()
        .await
        .expect("boots");

    assert!(
        private_timing(&app, true).await.is_some(),
        "the request the guard admits is stamped",
    );
    assert_eq!(
        private_timing(&app, false).await,
        None,
        "the guard chain's refusal is no oracle",
    );
}

/// A global-pool interceptor sits outside the band that renders a handler's
/// `Err`, so its own refusal reaches the header's wrap still an `Err`.
#[injectable]
#[derive(Default)]
struct AdmitInterceptor;

impl Layer for AdmitInterceptor {}

#[async_trait]
impl Interceptor for AdmitInterceptor {
    async fn intercept(
        &self,
        req: Request,
        next: Next<'_>,
    ) -> nest_rs_http::poem::Result<Response> {
        if req.headers().contains_key(ADMIT) {
            next.run(req).await
        } else {
            Err(nest_rs_http::poem::Error::from_status(
                StatusCode::UNAUTHORIZED,
            ))
        }
    }
}

#[module(imports = [ServerTimingModule], providers = [PrivateController, AdmitInterceptor])]
struct GatedModule;

#[tokio::test]
async fn a_pool_interceptor_refusal_carries_no_timing() {
    let app = TestApp::builder()
        .module::<GatedModule>()
        .use_interceptors_global([interceptor::<AdmitInterceptor>()])
        .build()
        .await
        .expect("boots");

    assert!(
        private_timing(&app, true).await.is_some(),
        "the request the interceptor admits is stamped",
    );
    assert_eq!(
        private_timing(&app, false).await,
        None,
        "an interceptor's `Err` refusal is no oracle",
    );
}

/// How many endpoint wraps the bare import attaches, booted in the process's
/// profile.
async fn bare_wraps() -> usize {
    let app = TestApp::for_module::<BareModule>()
        .await
        .expect("the import boots");
    Discovery::new(app.container())
        .meta::<HttpEndpointWrap>()
        .len()
}

#[test]
fn a_switched_off_header_mounts_nothing() {
    assert_eq!(
        in_profile("production", &[], bare_wraps),
        0,
        "off, the import attaches no wrap: a request pays nothing for it",
    );
    assert_eq!(
        in_profile("development", &[], bare_wraps),
        1,
        "on, the import attaches its one wrap",
    );
}
