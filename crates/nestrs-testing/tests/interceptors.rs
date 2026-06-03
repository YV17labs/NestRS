//! Per-handler / per-controller interceptor binding + guard-before-interceptor
//! ordering, end-to-end through the HTTP harness.

use nestrs_core::{injectable, module};
use nestrs_http::{async_trait, controller, routes, Guard, Interceptor, Next};
use nestrs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Request, Response, Result};

#[injectable]
#[derive(Default)]
struct Tracer;

#[async_trait]
impl Interceptor for Tracer {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let mut resp = next.run(req).await?;
        resp.headers_mut()
            .insert("x-trace", "hit".parse().expect("static header value"));
        Ok(resp)
    }
}

#[injectable]
#[derive(Default)]
struct DenyGuard;

#[async_trait]
impl Guard for DenyGuard {
    async fn check(&self, _req: &mut Request) -> std::result::Result<(), Response> {
        Err(Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body("denied"))
    }
}

#[controller(path = "/a")]
struct PerHandlerController;

#[routes]
impl PerHandlerController {
    #[get("/traced")]
    #[use_interceptors(Tracer)]
    async fn traced(&self) -> &'static str {
        "ok"
    }

    #[get("/plain")]
    async fn plain(&self) -> &'static str {
        "ok"
    }

    #[get("/denied")]
    #[use_guards(DenyGuard)]
    #[use_interceptors(Tracer)]
    async fn denied(&self) -> &'static str {
        "unreachable"
    }
}

#[controller(path = "/b")]
#[use_interceptors(Tracer)]
struct PerControllerController;

#[routes]
impl PerControllerController {
    #[get("/one")]
    async fn one(&self) -> &'static str {
        "ok"
    }

    #[get("/two")]
    async fn two(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [Tracer, DenyGuard, PerHandlerController, PerControllerController])]
struct InterceptorModule;

#[tokio::test]
async fn per_handler_interceptor_stamps_only_its_route() {
    let app = TestApp::for_module::<InterceptorModule>()
        .await
        .expect("boots");

    let traced = app.http().get("/a/traced").send().await;
    traced.assert_status_is_ok();
    traced.assert_header("x-trace", "hit");

    let plain = app.http().get("/a/plain").send().await;
    plain.assert_status_is_ok();
    plain.assert_header_is_not_exist("x-trace");
}

#[tokio::test]
async fn per_controller_interceptor_stamps_every_route() {
    let app = TestApp::for_module::<InterceptorModule>()
        .await
        .expect("boots");

    for path in ["/b/one", "/b/two"] {
        let resp = app.http().get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_header("x-trace", "hit");
    }
}

#[tokio::test]
async fn guard_short_circuits_before_the_interceptor() {
    let app = TestApp::for_module::<InterceptorModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/a/denied").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    resp.assert_header_is_not_exist("x-trace");
}
