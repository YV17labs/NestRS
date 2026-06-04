//! Bundled Swagger UI and spec endpoint. Assets are vendored under `assets/`
//! and embedded; `index.html` references them and the spec by absolute path
//! (`/api/...`, `/api-json`), matching the mount paths in [`crate::module`].

use poem::endpoint::make_sync;
use poem::web::Html;
use poem::{Endpoint, Response, handler};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const SWAGGER_CSS: &[u8] = include_bytes!("../assets/swagger-ui.css");
const SWAGGER_BUNDLE_JS: &[u8] = include_bytes!("../assets/swagger-ui-bundle.js");
const SWAGGER_PRESET_JS: &[u8] = include_bytes!("../assets/swagger-ui-standalone-preset.js");

#[handler]
pub fn swagger_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[handler]
pub fn swagger_css() -> Response {
    asset("text/css", SWAGGER_CSS)
}

#[handler]
pub fn swagger_bundle() -> Response {
    asset("application/javascript", SWAGGER_BUNDLE_JS)
}

#[handler]
pub fn swagger_preset() -> Response {
    asset("application/javascript", SWAGGER_PRESET_JS)
}

pub fn spec_endpoint(spec: String) -> impl Endpoint {
    make_sync(move |_req| {
        Response::builder()
            .content_type("application/json")
            .body(spec.clone())
    })
}

fn asset(content_type: &'static str, body: &'static [u8]) -> Response {
    // `body` is `&'static [u8]`, so `.body` wraps it via `Bytes::from_static` —
    // no per-request copy of the ~1.5 MB bundle.
    Response::builder()
        .content_type(content_type)
        .header("cache-control", "public, max-age=31536000, immutable")
        .body(body)
}
