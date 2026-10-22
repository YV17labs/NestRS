//! `HttpConfig.cors` on the wire, black-box through the transport: the
//! preflight, the headers an answer carries, and the refusal of an origin the
//! policy does not list. Whatever library the transport adapts, these hold.

use std::time::Duration;

use nest_rs_core::{ContainerBuilder, Discoverable, module};
use nest_rs_http::{
    HttpConfig, HttpCors, HttpEndpointMeta, HttpTransport, Response, controller, endpoint_fn,
    routes,
};
use nest_rs_http::{Method, StatusCode, header};
use nest_rs_testing::LogCapture;

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

#[controller(path = "/documents")]
struct DocumentsController;

#[routes]
impl DocumentsController {
    #[get("/{id}")]
    #[public]
    async fn read(&self) -> &'static str {
        "{}"
    }

    #[put("/{id}")]
    #[public]
    async fn replace(&self) -> &'static str {
        "{}"
    }

    #[delete("/{id}")]
    #[public]
    async fn remove(&self) -> &'static str {
        ""
    }
}

#[module(providers = [DocumentsController])]
struct DocumentsModule;

/// A self-mount answering every method itself, as MCP's endpoint does.
struct EveryMethodMount;

impl Discoverable for EveryMethodMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<EveryMethodMount, HttpEndpointMeta>(
            HttpEndpointMeta::new(&crate::PUBLIC_MOUNT, "/rpc", "rpc", |_container, routes| {
                routes.route_any(
                    "/rpc",
                    endpoint_fn(|_| async { Ok(Response::builder().body("rpc")) }),
                );
            })
            .owned_by("EveryMethodMount"),
        )
    }
}

#[module(providers = [EveryMethodMount])]
struct EveryMethodModule;

/// The methods WHATWG Fetch forbids a page to send: never one a policy lists.
const FORBIDDEN: [&str; 3] = ["connect", "trace", "track"];

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

/// A policy naming no method lists what the routes serve — and the
/// CORS-safelisted three, which a preflight still checks — never a method a
/// page cannot send; a method no route serves is refused.
#[tokio::test]
async fn a_policy_naming_no_method_lists_the_methods_the_app_routes() {
    let client = crate::boot_on::<DocumentsModule>(policy(HttpCors {
        origins: vec![LISTED.into()],
        ..HttpCors::default()
    }))
    .await;

    let preflight = client
        .request(Method::OPTIONS, "/documents/7")
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
        .send()
        .await;
    assert!(
        preflight.0.status().is_success(),
        "{}",
        preflight.0.status()
    );
    let mut methods = values(&preflight.0, header::ACCESS_CONTROL_ALLOW_METHODS);
    methods.sort();
    assert_eq!(methods, ["delete", "get", "head", "post", "put"]);

    let unrouted = client
        .request(Method::OPTIONS, "/documents/7")
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PATCH")
        .send()
        .await;
    unrouted.assert_status(StatusCode::FORBIDDEN);
}

/// An address answering every method itself admits every method a page sends
/// under a policy naming none — never a forbidden one.
#[tokio::test]
async fn a_mount_answering_every_method_admits_every_method_a_page_sends() {
    let client = crate::boot_on::<EveryMethodModule>(policy(HttpCors {
        origins: vec![LISTED.into()],
        ..HttpCors::default()
    }))
    .await;

    let preflight = client
        .request(Method::OPTIONS, "/rpc")
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE")
        .send()
        .await;
    assert!(
        preflight.0.status().is_success(),
        "{}",
        preflight.0.status()
    );
    let mut methods = values(&preflight.0, header::ACCESS_CONTROL_ALLOW_METHODS);
    methods.sort();
    assert_eq!(
        methods,
        ["delete", "get", "head", "options", "patch", "post", "put"]
    );
}

/// The wildcard admits every method, and says so as Fetch spells it, `*`,
/// rather than as a list naming a method no page can send.
#[tokio::test]
async fn a_wildcard_method_list_answers_the_wildcard_and_admits_an_extension_method() {
    let client = crate::boot_on::<CorsModule>(policy(HttpCors {
        origins: vec![LISTED.into()],
        methods: vec!["*".into()],
        ..HttpCors::default()
    }))
    .await;

    let preflight = client
        .request(Method::OPTIONS, "/cors")
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PROPFIND")
        .send()
        .await;
    assert!(
        preflight.0.status().is_success(),
        "{}",
        preflight.0.status()
    );
    let methods = values(&preflight.0, header::ACCESS_CONTROL_ALLOW_METHODS);
    assert_eq!(methods, ["*"]);
    assert!(
        !methods.iter().any(|m| FORBIDDEN.contains(&m.as_str())),
        "{methods:?}"
    );
}

/// A method a page cannot send is no method a policy can admit: the boot
/// refuses it, naming the rule.
#[test]
fn a_policy_listing_a_forbidden_method_fails_the_boot() {
    for method in ["TRACE", "connect", "Track"] {
        let refused = HttpTransport::from_config(&policy(HttpCors {
            origins: vec![LISTED.into()],
            methods: vec!["GET".into(), method.into()],
            ..HttpCors::default()
        }))
        .err()
        .unwrap_or_else(|| panic!("{method} is refused"));
        let said = format!("{refused:#}");
        assert!(said.contains("forbidden method"), "{said}");
        assert!(said.contains(&method.to_ascii_uppercase()), "{said}");
    }
}

/// Each refusal files one `warn` on the transport's target, naming the check
/// it failed and none of what the request sent.
#[tokio::test]
async fn each_refusal_files_one_warn_naming_its_check_and_no_value() {
    let client = crate::boot_on::<CorsModule>(policy(listed())).await;
    let cases = [
        (
            "origin_not_allowed",
            Method::GET,
            "https://evil.example",
            None,
            None,
        ),
        (
            "method_not_allowed",
            Method::OPTIONS,
            LISTED,
            Some("DELETE"),
            None,
        ),
        (
            "headers_not_allowed",
            Method::OPTIONS,
            LISTED,
            Some("PUT"),
            Some("x-evil-header"),
        ),
    ];
    for (reason, method, origin, requested, headers) in cases {
        let logs = LogCapture::install();
        let mut request = client
            .request(method, "/cors")
            .header(header::ORIGIN, origin);
        if let Some(requested) = requested {
            request = request.header(header::ACCESS_CONTROL_REQUEST_METHOD, requested);
        }
        if let Some(headers) = headers {
            request = request.header(header::ACCESS_CONTROL_REQUEST_HEADERS, headers);
        }
        request.send().await.assert_status(StatusCode::FORBIDDEN);

        let line = logs.expect_one(
            nest_rs_http::target::HTTP,
            "cross-origin request refused by the CORS policy",
        );
        assert_eq!(line.level, "warn", "{line:#?}");
        assert_eq!(line.field("reason").as_deref(), Some(reason), "{line:#?}");
        for event in logs.events() {
            let said = format!("{event:?}");
            assert!(!said.contains("evil"), "{said}");
            assert!(!said.contains("DELETE"), "{said}");
        }
    }
}
