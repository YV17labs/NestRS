//! Bundled Swagger UI and spec endpoint. `index.html` references the spec and
//! assets by **relative** path, so they resolve under any `global_prefix`.

use nest_rs_http::{Bytes, Endpoint, HeaderValue, Response, endpoint_fn, header};

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

const HTML: HeaderValue = HeaderValue::from_static("text/html; charset=utf-8");
const CSS: HeaderValue = HeaderValue::from_static("text/css");
const JAVASCRIPT: HeaderValue = HeaderValue::from_static("application/javascript");
const JSON: HeaderValue = HeaderValue::from_static("application/json");

/// The bundled assets never change under one build: a year, never revalidated.
const IMMUTABLE: HeaderValue = HeaderValue::from_static("public, max-age=31536000, immutable");

pub(crate) fn swagger_index() -> impl Endpoint {
    // Kept under a deployment's own policy: the edge's defaults fill only what a
    // response leaves unset.
    endpoint_fn(|_req| async {
        Ok(Response::builder()
            .content_type(HTML)
            .header(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(DOCS_CONTENT_SECURITY_POLICY),
            )
            .body(Bytes::from_static(INDEX_HTML.as_bytes())))
    })
}

pub(crate) fn swagger_css() -> impl Endpoint {
    asset(CSS, SWAGGER_CSS.as_bytes())
}

pub(crate) fn swagger_initializer() -> impl Endpoint {
    asset(JAVASCRIPT, SWAGGER_INITIALIZER_JS)
}

pub(crate) fn swagger_bundle() -> impl Endpoint {
    asset(JAVASCRIPT, SWAGGER_BUNDLE_JS)
}

pub(crate) fn swagger_preset() -> impl Endpoint {
    asset(JAVASCRIPT, SWAGGER_PRESET_JS)
}

/// The document, serialized once at mount and shared by every request.
pub(crate) fn spec_endpoint(spec: String) -> impl Endpoint {
    let spec = Bytes::from(spec);
    endpoint_fn(move |_req| {
        let spec = spec.clone();
        async move { Ok(Response::builder().content_type(JSON).body(spec)) }
    })
}

fn asset(content_type: HeaderValue, body: &'static [u8]) -> impl Endpoint {
    endpoint_fn(move |_req| {
        let content_type = content_type.clone();
        async move { Ok(cached(content_type, body)) }
    })
}

fn cached(content_type: HeaderValue, body: &'static [u8]) -> Response {
    Response::builder()
        .content_type(content_type)
        .header(header::CACHE_CONTROL, IMMUTABLE)
        .body(Bytes::from_static(body))
}

#[cfg(test)]
mod tests {
    use nest_rs_http::{Request, StatusCode};

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

    #[tokio::test]
    async fn asset_response_sets_long_lived_cache_header() {
        let resp = asset(CSS, b"body{}")
            .call(Request::builder().finish())
            .await
            .expect("an asset answers");
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

    #[tokio::test]
    async fn the_document_is_served_as_serialized() {
        let document = r#"{"openapi":"3.1.0"}"#;
        let resp = spec_endpoint(document.into())
            .call(Request::builder().finish())
            .await
            .expect("the document answers");
        assert_eq!(resp.headers()["content-type"], "application/json");
        assert_eq!(
            resp.into_body().into_string().await.expect("a body"),
            document
        );
    }
}
