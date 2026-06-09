//! Route-level HTTP decorators — `#[http_code]`, `#[response_header]`, `#[redirect]`,
//! and `Result<T, E>` error paths. Boots a real controller through `HttpTransport`.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, controller, routes};
use poem::error::ResponseError;
use poem::http::{StatusCode, header};
use poem::test::TestClient;
use poem::{IntoResponse, Response};

#[controller(path = "/")]
struct DecoratorProbeController;

#[routes]
impl DecoratorProbeController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        "Hello World"
    }

    #[post("/echo")]
    #[http_code(201)]
    #[response_header("x-powered-by", "nestrs")]
    async fn echo(&self) -> &'static str {
        "Hello World"
    }

    #[get("/docs")]
    #[redirect("https://docs.nestrs.dev", 301)]
    #[allow(dead_code, reason = "body discarded by #[redirect]")]
    async fn docs(&self) {}

    #[post("/forbidden")]
    #[http_code(201)]
    async fn forbidden(&self) -> Result<&'static str, ForbiddenError> {
        Err(ForbiddenError)
    }

    #[get("/xml-as-json")]
    #[response_header("content-type", "application/json")]
    async fn xml_as_json(&self) -> Response {
        let mut resp = "<root/>".into_response();
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            poem::http::HeaderValue::from_static("text/xml"),
        );
        resp
    }
}

#[derive(Debug)]
struct ForbiddenError;

impl std::fmt::Display for ForbiddenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("forbidden")
    }
}

impl std::error::Error for ForbiddenError {}

impl ResponseError for ForbiddenError {
    fn status(&self) -> StatusCode {
        StatusCode::FORBIDDEN
    }
}

#[module(providers = [DecoratorProbeController])]
struct DecoratorProbeModule;

async fn boot() -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    let app = App::builder()
        .module::<DecoratorProbeModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new();
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    let endpoint = transport
        .take_endpoint()
        .expect("configure populates the endpoint");
    TestClient::new(endpoint)
}

#[tokio::test]
async fn hello_endpoint_greets() {
    let client = boot().await;
    let resp = client.get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Hello World").await;
}

#[tokio::test]
async fn http_code_overrides_status_and_response_header_is_appended() {
    let client = boot().await;
    let resp = client.post("/echo").send().await;
    resp.assert_status(StatusCode::CREATED);
    resp.assert_header("x-powered-by", "nestrs");
    resp.assert_text("Hello World").await;
}

#[tokio::test]
async fn redirect_emits_status_and_location_header() {
    let client = boot().await;
    let resp = client.get("/docs").send().await;
    resp.assert_status(StatusCode::MOVED_PERMANENTLY);
    resp.assert_header("location", "https://docs.nestrs.dev");
}

#[tokio::test]
async fn http_code_does_not_override_the_status_of_err_responses() {
    let client = boot().await;
    let resp = client.post("/forbidden").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn response_header_overrides_a_handler_set_header() {
    let client = boot().await;
    let resp = client.get("/xml-as-json").send().await;
    resp.assert_status_is_ok();
    resp.assert_header_all("content-type", ["application/json"]);
}
