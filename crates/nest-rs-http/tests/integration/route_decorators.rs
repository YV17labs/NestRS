//! Route-level HTTP decorators — `#[http_code]`, `#[response_header]`, `#[redirect]`,
//! and `Result<T, E>` error paths. Boots a real controller through `HttpTransport`.

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use poem::Result as PoemResult;
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

    // No `#[allow(dead_code)]`: the macro emits it, and this suite compiles
    // under `-D warnings`.
    #[get("/docs")]
    #[redirect("https://docs.nestrs.dev", 301)]
    async fn docs(&self) {}

    #[post("/forbidden")]
    #[http_code(201)]
    async fn forbidden(&self) -> Result<&'static str, ForbiddenError> {
        Err(ForbiddenError)
    }

    /// The same refusal through a `Result` renamed on import.
    #[post("/forbidden-by-import")]
    #[http_code(201)]
    #[response_header("x-created", "yes")]
    async fn forbidden_by_import(&self) -> PoemResult<&'static str> {
        Err(ForbiddenError.into())
    }

    /// Through a type alias whose error is only a `ResponseError`: no
    /// `Result<T, E>: IntoResponse` bound is asked of it.
    #[post("/forbidden-by-alias")]
    #[http_code(201)]
    async fn forbidden_by_alias(&self) -> Refusable<&'static str> {
        Err(ForbiddenError)
    }

    #[post("/created-by-alias")]
    #[http_code(201)]
    #[response_header("x-created", "yes")]
    async fn created_by_alias(&self) -> Refusable<&'static str> {
        Ok("created")
    }

    /// The shapers written path-qualified, as an exported attribute macro may be.
    #[post("/qualified")]
    #[nest_rs_http::http_code(201)]
    #[nest_rs_http::response_header("x-probe", "yes")]
    async fn qualified(&self) -> &'static str {
        "created"
    }

    #[get("/qualified-redirect")]
    #[nest_rs_http::redirect("https://example.com", 301)]
    async fn qualified_redirect(&self) {}

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

/// A `Result` under another name.
type Refusable<T> = Result<T, ForbiddenError>;

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
    crate::boot::<DecoratorProbeModule>().await
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

/// A failure is known by its type, however it is spelled: an `Err` keeps its
/// `ResponseError` status and gets none of the success path's headers.
#[tokio::test]
async fn http_code_does_not_override_an_err_whatever_its_result_is_called() {
    let client = boot().await;
    for path in ["/forbidden-by-import", "/forbidden-by-alias"] {
        let resp = client.post(path).send().await;
        resp.assert_status(StatusCode::FORBIDDEN);
        resp.assert_header_is_not_exist("x-created");
    }
    let resp = client.post("/created-by-alias").send().await;
    resp.assert_status(StatusCode::CREATED);
    resp.assert_header("x-created", "yes");
    resp.assert_text("created").await;
}

#[tokio::test]
async fn response_header_overrides_a_handler_set_header() {
    let client = boot().await;
    let resp = client.get("/xml-as-json").send().await;
    resp.assert_status_is_ok();
    resp.assert_header_all("content-type", ["application/json"]);
}

#[tokio::test]
async fn a_path_qualified_shaper_shapes_the_response() {
    let client = boot().await;
    let resp = client.post("/qualified").send().await;
    resp.assert_status(StatusCode::CREATED);
    resp.assert_header("x-probe", "yes");

    let resp = client.get("/qualified-redirect").send().await;
    resp.assert_status(StatusCode::MOVED_PERMANENTLY);
    resp.assert_header(header::LOCATION, "https://example.com");
}
