//! The line a request files on `nest_rs::operation`.

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use nest_rs_testing::LogCapture;

use crate::boot;

#[controller(path = "/greetings")]
struct GreetingsController;

#[routes]
impl GreetingsController {
    #[get("/")]
    #[public]
    async fn greet(&self) -> &'static str {
        "hello"
    }
}

#[module(providers = [GreetingsController])]
struct GreetingsModule;

#[tokio::test]
async fn the_line_carries_no_client_address() {
    let logs = LogCapture::install();
    let client = boot::<GreetingsModule>().await;

    client.get("/greetings").send().await.assert_status_is_ok();

    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    assert_eq!(line.field("client_ip"), None, "{:?}", line.fields);
    assert_eq!(line.field("forwarded"), None, "{:?}", line.fields);
}

/// The line carries the server's values only: a `User-Agent` is free text the
/// client writes, and a method outside the known set is a token it chose.
#[tokio::test]
async fn the_line_carries_no_value_the_client_chose() {
    let logs = LogCapture::install();
    let client = boot::<GreetingsModule>().await;

    client
        .request(
            nest_rs_http::Method::from_bytes(b"MARKED").expect("a method token"),
            "/greetings",
        )
        .header("user-agent", "agent-marked/1.0")
        .send()
        .await;

    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    for (field, value) in &line.fields {
        assert!(
            !value.to_ascii_lowercase().contains("marked"),
            "{field} = {value}"
        );
    }
    assert_eq!(line.field("method").as_deref(), Some("_OTHER"));
    assert_eq!(line.field("user_agent"), None, "{:?}", line.fields);
}

/// A method the HTTP conventions know is named as sent.
#[tokio::test]
async fn the_line_names_a_known_method() {
    let logs = LogCapture::install();
    let client = boot::<GreetingsModule>().await;

    client.get("/greetings").send().await.assert_status_is_ok();

    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    assert_eq!(line.field("method").as_deref(), Some("GET"));
}

/// A self-mount answering a method outside the conventions' set, as a WebDAV
/// mount answers `PROPFIND`.
struct DavMount;

impl nest_rs_core::Discoverable for DavMount {
    fn register(builder: nest_rs_core::ContainerBuilder) -> nest_rs_core::ContainerBuilder {
        builder.attach_meta::<DavMount, nest_rs_http::HttpEndpointMeta>(
            nest_rs_http::HttpEndpointMeta::new(
                &crate::PUBLIC_MOUNT,
                "/dav",
                "dav",
                |_c, routes| {
                    routes.route(
                        nest_rs_http::Method::from_bytes(b"PROPFIND").expect("a method token"),
                        "/dav",
                        crate::answering("listed"),
                    );
                },
            )
            .owned_by("DavMount"),
        )
    }
}

#[module(providers = [DavMount])]
struct DavModule;

/// A method a route registers is the server's own word: the line and the span
/// name it as the route does, and the override the HTTP conventions ask of an
/// instrumentation that could turn a valid method into `_OTHER` is that route.
/// Any other extension method is still `_OTHER`.
#[tokio::test]
async fn a_method_a_route_registers_is_named_as_registered() {
    for (sent, named) in [("PROPFIND", "PROPFIND"), ("MARKED", "_OTHER")] {
        let logs = LogCapture::install();
        let client = boot::<DavModule>().await;

        client
            .request(
                nest_rs_http::Method::from_bytes(sent.as_bytes()).expect("a method token"),
                "/dav",
            )
            .send()
            .await;

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            nest_rs_http::unit::REQUEST.name(),
        );
        assert_eq!(line.field("method").as_deref(), Some(named), "{sent}");
        let span = logs.expect_span("nest_rs::http", "http.request");
        assert_eq!(
            span.field("http.request.method").as_deref(),
            Some(named),
            "{sent}"
        );
        assert_eq!(
            span.field("http.request.method_original").is_some(),
            named == nest_rs_core::operation_log::OTHER,
            "{sent}: {:?}",
            span.fields,
        );
    }
}
