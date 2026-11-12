//! `HttpConfig.cors` on the wire, black-box through the transport: the
//! preflight, the headers an answer carries, and the refusal of an origin the
//! policy does not list. Whatever library the transport adapts, these hold.

use std::time::Duration;

use nest_rs_core::{ContainerBuilder, Discoverable, module};
use nest_rs_http::__private::{OriginCheck, checks_origin};
use nest_rs_http::{HttpConfig, HttpCors, HttpEndpointMeta, HttpTransport, controller, routes};
use nest_rs_http::{Method, StatusCode, header};
use nest_rs_testing::{LogCapture, TestResponse};

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
                routes.route_any("/rpc", crate::answering("rpc"));
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

fn origin_only() -> HttpCors {
    HttpCors {
        origins: vec![LISTED.into()],
        ..HttpCors::default()
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

/// A preflight from the listed origin asking for `method` at `path`.
async fn preflight(client: &crate::Client, path: &str, method: &str) -> TestResponse {
    client
        .request(Method::OPTIONS, path)
        .header(header::ORIGIN, LISTED)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
        .send()
        .await
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

/// A policy that sets no lifetime lets the browser keep its own, a few
/// seconds, so a change to the policy reaches every page at once.
#[tokio::test]
async fn a_preflight_under_a_policy_with_no_lifetime_names_none() {
    let client = crate::boot_on::<CorsModule>(policy(HttpCors {
        max_age: None,
        ..listed()
    }))
    .await;

    let preflight = preflight(&client, "/cors", "PUT").await;

    assert!(
        preflight.0.status().is_success(),
        "{}",
        preflight.0.status()
    );
    preflight.assert_header_is_not_exist(header::ACCESS_CONTROL_MAX_AGE);
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

    let refused = preflight(&client, "/cors", "DELETE").await;

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

/// An answer to any origin varies on it beside what compression varies on: a
/// shared cache keys on both, or hands one client a coding it never accepted.
#[tokio::test]
async fn an_answer_to_any_origin_varies_on_it_and_on_what_compression_reads() {
    let client = crate::boot_on::<CorsModule>(HttpConfig {
        compression: true,
        ..policy(HttpCors {
            origins: vec!["*".into()],
            ..HttpCors::default()
        })
    })
    .await;

    let read = client
        .get("/cors")
        .header(header::ORIGIN, "https://any.example")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;

    read.assert_status_is_ok();
    read.assert_header(header::CONTENT_ENCODING, "gzip");
    let vary = values(&read.0, header::VARY);
    for name in ["origin", "accept-encoding"] {
        assert!(vary.iter().any(|v| v == name), "{name}: {vary:?}");
    }
}

/// A policy naming no method lists what the routes serve — and the
/// CORS-safelisted three, which a preflight still checks — never a method a
/// page cannot send; a method no route serves is refused.
#[tokio::test]
async fn a_policy_naming_no_method_lists_the_methods_the_app_routes() {
    let client = crate::boot_on::<DocumentsModule>(policy(origin_only())).await;

    let routed = preflight(&client, "/documents/7", "PUT").await;
    assert!(routed.0.status().is_success(), "{}", routed.0.status());
    let mut methods = values(&routed.0, header::ACCESS_CONTROL_ALLOW_METHODS);
    methods.sort();
    assert_eq!(methods, ["delete", "get", "head", "post", "put"]);

    let unrouted = preflight(&client, "/documents/7", "PATCH").await;
    unrouted.assert_status(StatusCode::FORBIDDEN);
}

/// An address answering every method itself admits every method a page sends
/// under a policy naming none — never a forbidden one.
#[tokio::test]
async fn a_mount_answering_every_method_admits_every_method_a_page_sends() {
    let client = crate::boot_on::<EveryMethodModule>(policy(origin_only())).await;

    let preflight = preflight(&client, "/rpc", "DELETE").await;
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

/// A self-mount with an entry for `OPTIONS` itself, beside one answering every
/// method: what a plain `OPTIONS` from a page is sent to.
struct OptionsMount;

impl Discoverable for OptionsMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<OptionsMount, HttpEndpointMeta>(
            HttpEndpointMeta::new(
                &crate::PUBLIC_MOUNT,
                "/",
                "options",
                |_container, routes| {
                    routes.route(
                        Method::OPTIONS,
                        "/capabilities",
                        crate::answering("options"),
                    );
                    routes.route_any("/rpc", crate::answering("rpc"));
                },
            )
            .owned_by("OptionsMount"),
        )
    }
}

#[module(providers = [OptionsMount, PagesController])]
struct OptionsModule;

/// A page's own `fetch(url, { method: "OPTIONS" })` carries no
/// `Access-Control-Request-Method`, the request its preflight admitted: it
/// reaches the entry that answers it, and the page may read the answer.
#[tokio::test]
async fn a_plain_options_from_a_listed_origin_reaches_what_answers_it() {
    let client = crate::boot_on::<OptionsModule>(policy(origin_only())).await;

    for (path, body) in [("/capabilities", "options"), ("/rpc", "rpc")] {
        let answered = client
            .request(Method::OPTIONS, path)
            .header(header::ORIGIN, LISTED)
            .send()
            .await;
        answered.assert_status_is_ok();
        answered.assert_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, LISTED);
        answered.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_METHODS);
        let vary = values(&answered.0, header::VARY);
        assert!(vary.iter().any(|v| v == "origin"), "{vary:?}");
        answered.assert_text(body).await;
    }

    let unrouted = client
        .request(Method::OPTIONS, "/cors")
        .header(header::ORIGIN, LISTED)
        .send()
        .await;
    unrouted.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    unrouted.assert_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, LISTED);
}

/// Only the origin check stands in front of it: an origin the policy does not
/// list reaches nothing, whatever the method.
#[tokio::test]
async fn a_plain_options_from_an_unlisted_origin_is_refused() {
    let client = crate::boot_on::<OptionsModule>(policy(origin_only())).await;

    let refused = client
        .request(Method::OPTIONS, "/capabilities")
        .header(header::ORIGIN, "https://evil.example")
        .send()
        .await;
    refused.assert_status(StatusCode::FORBIDDEN);
    refused.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
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

    let preflight = preflight(&client, "/cors", "PROPFIND").await;
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

/// An origin no header can carry is a configuration the boot refuses, naming
/// the list and the place, never a panic.
#[test]
fn a_policy_listing_an_origin_no_header_carries_fails_the_boot() {
    let refused = HttpTransport::from_config(&policy(HttpCors {
        origins: vec!["https://app.example\r\n".into()],
        ..HttpCors::default()
    }))
    .err()
    .expect("the origin is refused");
    let said = format!("{refused:#}");
    assert!(said.contains("entry 1 of `origins`"), "{said}");
    assert!(!said.contains("app.example"), "{said}");
}

/// A self-mount whose every request is origin-checked, as `/mcp` is: the
/// list's other reader.
struct CheckedMount;

impl Discoverable for CheckedMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<CheckedMount, HttpEndpointMeta>(checks_origin(
            HttpEndpointMeta::new(
                &crate::PUBLIC_MOUNT,
                "/checked",
                "checked",
                |_container, routes| {
                    routes.route_any("/checked", crate::answering("checked"));
                },
            )
            .owned_by("CheckedMount"),
            OriginCheck::Every,
            |_refused| {},
        ))
    }
}

#[module(providers = [PagesController, CheckedMount])]
struct BothReadersModule;

/// The verdict a route and the origin-checked mount give `origin`, as their
/// statuses.
async fn verdicts(client: &crate::Client, origin: &str) -> (StatusCode, StatusCode) {
    let route = client
        .get("/cors")
        .header(header::ORIGIN, origin)
        .send()
        .await;
    let checked = client
        .get("/checked")
        .header(header::ORIGIN, origin)
        .send()
        .await;
    (route.0.status(), checked.0.status())
}

/// One list, two readers: the CORS layer and the origin check give one
/// verdict for each `Origin`, an entry read as the origin it parses to, an
/// IPv4 shorthand (`127.1`) as the address a browser on it sends.
#[tokio::test]
async fn the_cors_layer_and_the_origin_check_read_one_list_alike() {
    let client = crate::boot_on::<BothReadersModule>(policy(HttpCors {
        origins: vec![
            "https://APP.example:443".into(),
            "http://local.example:8080".into(),
            "https://127.1".into(),
        ],
        ..HttpCors::default()
    }))
    .await;

    for (origin, expected) in [
        ("https://app.example", StatusCode::OK),
        ("HTTPS://App.Example:443", StatusCode::OK),
        ("http://local.example:8080", StatusCode::OK),
        ("http://local.example", StatusCode::FORBIDDEN),
        ("https://127.0.0.1", StatusCode::OK),
        ("https://127.0.0.2", StatusCode::FORBIDDEN),
        ("https://app.example:8443", StatusCode::FORBIDDEN),
        ("https://evil.example", StatusCode::FORBIDDEN),
        ("https://app.example/", StatusCode::FORBIDDEN),
        ("null", StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            verdicts(&client, origin).await,
            (expected, expected),
            "{origin}"
        );
    }
}

/// `null` is what any sandboxed or `data:` document sends: refused by both
/// readers under a list, admitted by both under the wildcard alone.
#[tokio::test]
async fn null_is_refused_by_both_readers_but_under_the_wildcard() {
    let listed = crate::boot_on::<BothReadersModule>(policy(origin_only())).await;
    assert_eq!(
        verdicts(&listed, "null").await,
        (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN)
    );
    let every = crate::boot_on::<BothReadersModule>(policy(HttpCors {
        origins: vec!["*".into()],
        ..HttpCors::default()
    }))
    .await;
    assert_eq!(
        verdicts(&every, "null").await,
        (StatusCode::OK, StatusCode::OK)
    );
}

/// A list naming no origin installs no CORS layer, whatever else the policy
/// says: a cross-origin page reads nothing, credentials or not.
#[tokio::test]
async fn a_policy_listing_no_origin_installs_no_cors_layer() {
    let client = crate::boot_on::<CorsModule>(policy(HttpCors {
        credentials: true,
        ..HttpCors::default()
    }))
    .await;

    let read = client
        .get("/cors")
        .header(header::ORIGIN, "https://evil.example")
        .send()
        .await;

    read.assert_status_is_ok();
    read.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    read.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
}

/// `null` in the list fails the boot by place, saying why.
#[test]
fn a_policy_listing_null_fails_the_boot() {
    let refused = HttpTransport::from_config(&policy(HttpCors {
        origins: vec![LISTED.into(), "null".into()],
        ..HttpCors::default()
    }))
    .err()
    .expect("`null` is refused");
    let said = format!("{refused:#}");
    assert!(said.contains("entry 2 of `origins`"), "{said}");
    assert!(!said.contains("app.example.com"), "{said}");
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
