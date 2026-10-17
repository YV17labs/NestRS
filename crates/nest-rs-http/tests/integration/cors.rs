//! `HttpConfig.cors` on the wire, black-box through the transport: the
//! preflight, the headers an answer carries, and the refusal of an origin the
//! policy does not list. Whatever library the transport adapts, these hold.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::{HttpConfig, HttpCors, controller, routes};
use nest_rs_http::{Method, StatusCode, header};

#[controller(path = "/cors")]
struct PagesController;

#[routes]
impl PagesController {
    #[get("/")]
    #[public]
    async fn page(&self) -> &'static str {
        "[]"
    }
}

#[module(providers = [PagesController])]
struct CorsModule;

const LISTED: &str = "https://app.example.com";

fn policy(cors: HttpCors) -> HttpConfig {
    HttpConfig {
        cors: Some(cors),
        ..HttpConfig::default()
    }
}

fn listed() -> HttpCors {
    HttpCors {
        origins: vec![LISTED.into()],
        methods: vec!["GET".into(), "PUT".into()],
        headers: vec!["x-tenant".into()],
        exposed_headers: vec!["x-request-cost".into()],
        max_age: Some(Duration::from_secs(600)),
        ..HttpCors::default()
    }
}

/// Every value of `name`, split on commas, lowercased.
fn values(response: &nest_rs_http::Response, name: header::HeaderName) -> Vec<String> {
    response
        .headers()
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(',').map(|v| v.trim().to_ascii_lowercase()))
        .collect()
}

#[tokio::test]
async fn a_preflight_from_a_listed_origin_answers_the_policy_and_varies_on_the_origin() {
    let client = crate::boot_on::<CorsModule>(policy(listed())).await;

    let preflight = client
        .request(Method::OPTIONS, "/cors")
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "x-tenant")
        .send()
        .await;

    assert!(
        preflight.0.status().is_success(),
        "{}",
        preflight.0.status()
    );
    preflight.assert_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, LISTED);
    preflight.assert_header(header::ACCESS_CONTROL_MAX_AGE, "600");
    let methods = values(&preflight.0, header::ACCESS_CONTROL_ALLOW_METHODS);
    assert!(methods.iter().any(|m| m == "put"), "{methods:?}");
    let headers = values(&preflight.0, header::ACCESS_CONTROL_ALLOW_HEADERS);
    assert!(headers.iter().any(|h| h == "x-tenant"), "{headers:?}");
    let vary = values(&preflight.0, header::VARY);
    assert!(vary.iter().any(|v| v == "origin"), "{vary:?}");
}

#[tokio::test]
async fn an_answer_to_a_listed_origin_names_it_exposes_what_the_framework_sends_and_varies_on_it() {
    let client = crate::boot_on::<CorsModule>(policy(listed())).await;

    let read = client
        .get("/cors")
        .header(header::ORIGIN, LISTED)
        .send()
        .await;

    read.assert_status_is_ok();
    read.assert_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, LISTED);
    let vary = values(&read.0, header::VARY);
    assert!(vary.iter().any(|v| v == "origin"), "{vary:?}");
    let exposed = values(&read.0, header::ACCESS_CONTROL_EXPOSE_HEADERS);
    for name in ["link", "location", "retry-after", "x-request-cost"] {
        assert!(
            exposed.iter().any(|h| h == name),
            "{name} is exposed: {exposed:?}"
        );
    }
}

#[tokio::test]
async fn a_request_from_an_origin_the_policy_does_not_list_answers_a_403_problem_document() {
    let client = crate::boot_on::<CorsModule>(policy(listed())).await;

    let refused = client
        .get("/cors")
        .header(header::ORIGIN, "https://evil.example")
        .send()
        .await;

    refused.assert_status(StatusCode::FORBIDDEN);
    refused.assert_content_type("application/problem+json");
    refused.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    let vary = values(&refused.0, header::VARY);
    assert!(
        vary.iter().any(|v| v == "origin"),
        "a refusal depends on the origin too: {vary:?}"
    );
}

#[tokio::test]
async fn a_refused_preflight_varies_on_the_origin() {
    let client = crate::boot_on::<CorsModule>(policy(listed())).await;

    let refused = client
        .request(Method::OPTIONS, "/cors")
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE")
        .send()
        .await;

    refused.assert_status(StatusCode::FORBIDDEN);
    refused.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    let vary = values(&refused.0, header::VARY);
    assert!(vary.iter().any(|v| v == "origin"), "{vary:?}");
}

#[tokio::test]
async fn a_request_without_an_origin_is_no_cors_request_and_is_answered() {
    let client = crate::boot_on::<CorsModule>(policy(listed())).await;

    let read = client.get("/cors").send().await;

    read.assert_status_is_ok();
    read.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
}

/// `*` is the wildcard each list documents, never a literal origin, method
/// or header name the request must spell.
#[tokio::test]
async fn a_wildcard_opens_every_origin_method_and_header() {
    let client = crate::boot_on::<CorsModule>(policy(HttpCors {
        origins: vec!["*".into()],
        methods: vec!["*".into()],
        headers: vec!["*".into()],
        ..HttpCors::default()
    }))
    .await;

    let read = client
        .get("/cors")
        .header(header::ORIGIN, "https://any.example")
        .send()
        .await;
    read.assert_status_is_ok();
    read.assert_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "https://any.example");
    let preflight = client
        .request(Method::OPTIONS, "/cors")
        .header(header::ORIGIN, "https://any.example")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "x-anything")
        .send()
        .await;
    assert!(
        preflight.0.status().is_success(),
        "{}",
        preflight.0.status()
    );
}
