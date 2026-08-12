//! `Header<T>` through a mounted route — the half `src/header.rs`'s unit tests
//! cannot reach.
//!
//! Those drive `from_request` directly. These drive a real request through
//! `#[routes]`, which is where the two things that actually break live: the
//! extractor running in the wrapper's emitted order, and the rejection reaching
//! the client as a status rather than a panic.

use nest_rs_core::module;
use nest_rs_http::{Header, Valid, controller, routes};
use poem::http::StatusCode;
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Trace {
    #[serde(rename = "X-Request-Id")]
    request_id: String,
    #[serde(rename = "X-Retry-Count")]
    retry: Option<u32>,
}

#[derive(Debug, Deserialize, Validate, schemars::JsonSchema)]
struct Tenant {
    #[serde(rename = "X-Tenant")]
    #[validate(length(min = 3))]
    tenant: String,
}

#[controller(path = "/headers")]
#[derive(Default)]
struct HeadersController;

#[routes]
impl HeadersController {
    #[get("/trace")]
    async fn trace(&self, trace: Header<Trace>) -> String {
        let trace = trace.into_inner();
        format!("{} {}", trace.request_id, trace.retry.unwrap_or_default())
    }

    /// The same carrier every other extractor validates through, so a header
    /// DTO gets edge validation without a second mechanism.
    #[get("/tenant")]
    async fn tenant(&self, tenant: Valid<Header<Tenant>>) -> String {
        tenant.into_inner().tenant
    }
}

#[module(providers = [HeadersController])]
struct HeadersApp;

#[tokio::test]
async fn a_header_dto_binds_from_the_request_headers() {
    let client = crate::boot::<HeadersApp>().await;
    let resp = client
        .get("/headers/trace")
        .header("x-request-id", "abc-123")
        .header("X-Retry-Count", "4")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("abc-123 4").await;
}

#[tokio::test]
async fn an_absent_optional_header_is_simply_absent() {
    let client = crate::boot::<HeadersApp>().await;
    let resp = client
        .get("/headers/trace")
        .header("X-Request-Id", "abc-123")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("abc-123 0").await;
}

#[tokio::test]
async fn a_missing_required_header_answers_400_naming_it() {
    let client = crate::boot::<HeadersApp>().await;
    let resp = client.get("/headers/trace").send().await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_content_type("application/problem+json");
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(
        body.contains("X-Request-Id"),
        "the rejection names the header the caller must send: {body}",
    );
}

#[tokio::test]
async fn a_header_dto_validates_through_the_shared_pipe_carrier() {
    let client = crate::boot::<HeadersApp>().await;
    client
        .get("/headers/tenant")
        .header("X-Tenant", "acme")
        .send()
        .await
        .assert_status_is_ok();

    let short = client
        .get("/headers/tenant")
        .header("X-Tenant", "ab")
        .send()
        .await;
    short.assert_status(StatusCode::BAD_REQUEST);
}

/// A `#[serde(rename)]` naming something that is not a header name.
///
/// `http` implements `AsHeaderName for &str` by failing the lookup, so
/// `HeaderMap::get` answers `None` — the same answer as "the caller did not send
/// it". An `Option<_>` field bound `None` on every request forever, and a
/// required one 400'd telling the caller to send a header no client can send.
/// Neither points at the `rename`, which is where the mistake is.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct BadName {
    #[serde(rename = "X Request Id")]
    request_id: Option<String>,
}

/// A flattened field. serde routes the whole struct through `deserialize_map`,
/// whose keys are the lowercased names `HeaderMap` stores, and matches
/// case-sensitively — so the wire name must be spelled lowercase here.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Flat {
    #[serde(rename = "x-request-id")]
    request_id: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Wrapper {
    #[serde(flatten)]
    inner: Flat,
}

#[controller(path = "/edge")]
struct EdgeController;

#[routes]
impl EdgeController {
    #[get("/bad-name")]
    async fn bad_name(&self, headers: Header<BadName>) -> String {
        headers.into_inner().request_id.unwrap_or_default()
    }

    #[get("/flat")]
    async fn flat(&self, headers: Header<Wrapper>) -> String {
        headers.into_inner().inner.request_id.unwrap_or_default()
    }
}

#[module(providers = [EdgeController])]
struct EdgeModule;

#[tokio::test]
async fn a_field_naming_something_that_is_not_a_header_is_refused() {
    let client = crate::boot::<EdgeModule>().await;
    let resp = client
        .get("/edge/bad-name")
        .header("X-Request-Id", "abc-123")
        .send()
        .await;
    resp.assert_status(poem::http::StatusCode::BAD_REQUEST);
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("is not a valid header name"),
        "the refusal points at the declaration, not at the caller: {body}",
    );
    assert!(
        !body.contains("abc-123"),
        "and it still never echoes a header value: {body}",
    );
}

#[tokio::test]
async fn a_flattened_field_matches_its_header_in_lowercase() {
    let client = crate::boot::<EdgeModule>().await;
    // Whatever case the client sends: `HeaderMap` stores it lowercased, which is
    // the key the flattened field is matched against.
    for sent in ["X-Request-Id", "x-request-id", "X-REQUEST-ID"] {
        let resp = client
            .get("/edge/flat")
            .header(sent, "abc-123")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("abc-123").await;
    }
}
