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

#[cfg(test)]
mod tests {
    use poem::http::StatusCode;

    use super::*;

    // The embedded index must point at the absolute mount paths the module
    // also mounts — a rename of either side without the other turns the UI
    // into a white page in prod. Pin the contract.
    #[test]
    fn embedded_index_references_the_absolute_mount_paths() {
        assert!(
            INDEX_HTML.contains("/api-json"),
            "index.html must reference the spec endpoint at /api-json",
        );
        assert!(
            INDEX_HTML.contains("/api/"),
            "index.html must reference assets under /api/",
        );
    }

    #[test]
    fn bundled_assets_are_not_empty() {
        // A vendored asset overwritten with an empty file would 200 a blank
        // page in prod — fail loud here instead.
        assert!(!SWAGGER_CSS.is_empty());
        assert!(!SWAGGER_BUNDLE_JS.is_empty());
        assert!(!SWAGGER_PRESET_JS.is_empty());
    }

    #[test]
    fn asset_response_sets_long_lived_cache_header() {
        let resp = asset("text/css", b"body{}");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("text/css"),
        );
        let cache = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(cache.contains("public"), "missing public: {cache}");
        assert!(cache.contains("immutable"), "missing immutable: {cache}");
        assert!(cache.contains("31536000"), "missing 1y max-age: {cache}");
    }

    #[test]
    fn spec_endpoint_constructs_without_panic() {
        // The endpoint trait is async; an integration test (with tokio) exercises
        // the body end-to-end via `TestClient`. Here we only check the
        // constructor does not panic on a representative spec — the closure
        // body is the trivial `body(spec.clone())` write.
        let _ = spec_endpoint(r#"{"openapi":"3.1.0"}"#.into());
    }
}
