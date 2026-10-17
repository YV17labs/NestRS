//! `Header<T>` through a mounted route: the extractor in the wrapper's emitted
//! order, and the rejection reaching the client as a status.

use nest_rs_core::module;
use nest_rs_http::StatusCode;
use nest_rs_http::{Header, Valid, controller, routes};
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

    /// A header DTO validated through the same carrier as every extractor.
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

/// A `#[serde(rename)]` naming something that is not a header name, which
/// `HeaderMap::get` would answer like an absent header.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct BadName {
    #[serde(rename = "X Request Id")]
    request_id: Option<String>,
}

/// A flattened field: serde matches the lowercased keys `HeaderMap` stores
/// case-sensitively, so the wire name must be spelled lowercase here.
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
    resp.assert_status(nest_rs_http::StatusCode::BAD_REQUEST);
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
    // `HeaderMap` stores whatever case the client sends lowercased.
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

/// Each route above on the vocabulary's extractors, answering what its 6.x
/// twin answers.
async fn on_the_vocabulary<T, F>(template: &'static str, answer: F) -> crate::Client
where
    T: serde::de::DeserializeOwned + Send + 'static,
    F: Fn(T) -> String + Copy + Send + Sync + 'static,
{
    crate::vocabulary_route(
        nest_rs_http::Method::GET,
        template,
        move |req, mut body| async move {
            let header =
                <Header<T> as nest_rs_http::FromRequest>::from_request(&req, &mut body).await?;
            Ok(nest_rs_http::IntoResponse::into_response(answer(
                header.into_inner(),
            )))
        },
    )
    .await
}

#[tokio::test]
async fn the_vocabulary_answers_each_case_as_6x_did() {
    type Sent = &'static [(&'static str, &'static str)];
    let trace = on_the_vocabulary::<Trace, _>("/headers/trace", |trace| {
        format!("{} {}", trace.request_id, trace.retry.unwrap_or_default())
    })
    .await;
    let tenant = crate::vocabulary_route(
        nest_rs_http::Method::GET,
        "/headers/tenant",
        |req, mut body| async move {
            let tenant =
                <Valid<Header<Tenant>> as nest_rs_http::FromRequest>::from_request(&req, &mut body)
                    .await?;
            Ok(nest_rs_http::IntoResponse::into_response(
                tenant.into_inner().tenant,
            ))
        },
    )
    .await;
    let bad_name = on_the_vocabulary::<BadName, _>("/edge/bad-name", |headers| {
        headers.request_id.unwrap_or_default()
    })
    .await;
    let flat = on_the_vocabulary::<Wrapper, _>("/edge/flat", |headers| {
        headers.inner.request_id.unwrap_or_default()
    })
    .await;
    let six_headers = crate::boot::<HeadersApp>().await;
    let six_edge = crate::boot::<EdgeModule>().await;

    let cases: [(&crate::Client, &crate::Client, &str, Sent); 9] = [
        (
            &trace,
            &six_headers,
            "/headers/trace",
            &[("x-request-id", "abc-123"), ("X-Retry-Count", "4")],
        ),
        (
            &trace,
            &six_headers,
            "/headers/trace",
            &[("X-Request-Id", "abc-123")],
        ),
        (&trace, &six_headers, "/headers/trace", &[]),
        (
            &trace,
            &six_headers,
            "/headers/trace",
            &[("X-Request-Id", "a"), ("X-Retry-Count", "sk_live_51")],
        ),
        (
            &tenant,
            &six_headers,
            "/headers/tenant",
            &[("X-Tenant", "acme")],
        ),
        (
            &tenant,
            &six_headers,
            "/headers/tenant",
            &[("X-Tenant", "ab")],
        ),
        (
            &bad_name,
            &six_edge,
            "/edge/bad-name",
            &[("X-Request-Id", "abc-123")],
        ),
        (
            &flat,
            &six_edge,
            "/edge/flat",
            &[("X-Request-Id", "abc-123")],
        ),
        (
            &flat,
            &six_edge,
            "/edge/flat",
            &[("X-REQUEST-ID", "abc-123")],
        ),
    ];
    for (seven, six, path, sent) in cases {
        let mut on_seven = seven.get(path);
        let mut on_six = six.get(path);
        for (name, value) in sent {
            on_seven = on_seven.header(*name, *value);
            on_six = on_six.header(*name, *value);
        }
        assert_eq!(
            crate::answered(on_seven.send().await).await,
            crate::answered(on_six.send().await).await,
            "{path} {sent:?}",
        );
    }
}
