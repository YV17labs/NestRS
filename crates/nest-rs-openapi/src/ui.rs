//! Bundled Swagger UI and spec endpoint. `index.html` references the spec and
//! assets by **relative** path, so they resolve under any `global_prefix`.

use poem::endpoint::make_sync;
use poem::{Endpoint, Response, handler};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const SWAGGER_CSS: &str = concat!(
    include_str!("../assets/swagger-ui.css"),
    "\n",
    include_str!("../assets/index.css"),
);
const SWAGGER_BUNDLE_JS: &[u8] = include_bytes!("../assets/swagger-ui-bundle.js");
const SWAGGER_PRESET_JS: &[u8] = include_bytes!("../assets/swagger-ui-standalone-preset.js");
const SWAGGER_INITIALIZER_JS: &[u8] = include_bytes!("../assets/swagger-initializer.js");

/// The documentation page loads only its own assets, fetches only its own
/// origin, and cannot be framed. Swagger UI's bundle applies inline styles, the
/// one inline source it needs.
pub(crate) const DOCS_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; \
     connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[handler]
pub(crate) fn swagger_index() -> Response {
    // A deployment's own `Content-Security-Policy`, stamped at the edge, replaces this one.
    Response::builder()
        .content_type("text/html; charset=utf-8")
        .header("content-security-policy", DOCS_CONTENT_SECURITY_POLICY)
        .body(INDEX_HTML)
}

#[handler]
pub(crate) fn swagger_css() -> Response {
    asset("text/css", SWAGGER_CSS.as_bytes())
}

#[handler]
pub(crate) fn swagger_initializer() -> Response {
    asset("application/javascript", SWAGGER_INITIALIZER_JS)
}

#[handler]
pub(crate) fn swagger_bundle() -> Response {
    asset("application/javascript", SWAGGER_BUNDLE_JS)
}

#[handler]
pub(crate) fn swagger_preset() -> Response {
    asset("application/javascript", SWAGGER_PRESET_JS)
}

pub(crate) fn spec_endpoint(spec: String) -> impl Endpoint {
    make_sync(move |_req| {
        Response::builder()
            .content_type("application/json")
            .body(spec.clone())
    })
}

fn asset(content_type: &'static str, body: &'static [u8]) -> Response {
    Response::builder()
        .content_type(content_type)
        .header("cache-control", "public, max-age=31536000, immutable")
        .body(body)
}

#[cfg(test)]
mod tests {
    use poem::http::StatusCode;

    use super::*;

    #[test]
    fn embedded_index_references_the_relative_mount_paths() {
        let initializer = std::str::from_utf8(SWAGGER_INITIALIZER_JS).expect("utf-8");
        assert!(
            initializer.contains("\"api-json\""),
            "the initializer must fetch the spec relatively (\"api-json\"), not \"/api-json\"",
        );
        assert!(
            INDEX_HTML.contains("\"api/swagger-ui.css\""),
            "index.html must reference the stylesheet relatively (api/swagger-ui.css)",
        );
        for page in [INDEX_HTML, initializer] {
            assert!(
                !page.contains("\"/api-json\"") && !page.contains("\"/api/"),
                "no absolute (leading-slash) references — they break under a global_prefix",
            );
        }
    }

    #[test]
    fn bundled_assets_are_not_empty() {
        assert!(!SWAGGER_CSS.is_empty());
        assert!(!SWAGGER_BUNDLE_JS.is_empty());
        assert!(!SWAGGER_PRESET_JS.is_empty());
        assert!(!SWAGGER_INITIALIZER_JS.is_empty());
    }

    #[test]
    fn the_page_carries_no_inline_script_its_policy_would_refuse() {
        assert!(
            !INDEX_HTML.contains("<script>") && !INDEX_HTML.contains("<style"),
            "every script and style is a file `script-src 'self'` admits: {INDEX_HTML}",
        );
        assert!(INDEX_HTML.contains("\"api/swagger-initializer.js\""));
        assert!(DOCS_CONTENT_SECURITY_POLICY.contains("script-src 'self';"));
        assert!(DOCS_CONTENT_SECURITY_POLICY.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn the_ui_never_reports_the_document_to_an_online_validator() {
        let initializer = std::str::from_utf8(SWAGGER_INITIALIZER_JS).expect("utf-8");
        assert!(
            initializer.contains("validatorUrl: \"none\""),
            "Swagger UI's default badge sends the document's URL to a third party: {initializer}",
        );
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
        let _ = spec_endpoint(r#"{"openapi":"3.1.0"}"#.into());
    }
}
