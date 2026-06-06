//! CORS via `HttpTransport::cors`: a configured poem `Cors` middleware wraps the
//! whole route tree, exercised end-to-end through the in-process harness.

use nest_rs_core::module;
use nest_rs_http::poem::http::Method;
use nest_rs_http::poem::middleware::Cors;
use nest_rs_http::{HttpTransport, controller, routes};
use nest_rs_testing::TestApp;

#[controller(path = "/")]
struct ThingController;

#[routes]
impl ThingController {
    #[get("/thing")]
    async fn thing(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [ThingController])]
struct CorsModule;

fn cors() -> Cors {
    Cors::new()
        .allow_origin("http://example.com")
        .allow_method(Method::GET)
}

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<CorsModule>()
        .http(HttpTransport::new().cors(cors()))
        .build()
        .await
        .expect("boots")
}

#[tokio::test]
async fn simple_request_carries_the_allow_origin_header() {
    let app = boot().await;
    let resp = app
        .http()
        .get("/thing")
        .header("Origin", "http://example.com")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header("access-control-allow-origin", "http://example.com");
}

#[tokio::test]
async fn preflight_is_answered_before_the_handler() {
    let app = boot().await;
    let resp = app
        .http()
        .request(Method::OPTIONS, "/thing")
        .header("Origin", "http://example.com")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header("access-control-allow-origin", "http://example.com");
}
