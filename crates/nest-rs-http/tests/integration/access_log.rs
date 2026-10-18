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

/// A unit of work whose every commit its database refuses.
struct Refusing;

struct RefusedUnit;

struct NoExecutor;

impl nest_rs_database::Executor for NoExecutor {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl nest_rs_database::OpenUnit for RefusedUnit {
    fn executor(&self) -> std::sync::Arc<dyn nest_rs_database::Executor> {
        std::sync::Arc::new(NoExecutor)
    }

    fn settle(
        self: Box<Self>,
        _ok: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = nest_rs_database::Settlement> + Send>>
    {
        Box::pin(std::future::ready(
            nest_rs_database::Settlement::Unhonoured(nest_rs_database::Unhonoured::deterministic(
                "refused by the test",
            )),
        ))
    }
}

impl nest_rs_database::UnitOfWork for Refusing {
    fn open(
        &self,
        _: &nest_rs_database::UnitSpec,
    ) -> Result<Box<dyn nest_rs_database::OpenUnit>, nest_rs_database::Unhonoured> {
        Ok(Box::new(RefusedUnit))
    }

    fn settle_net(&self) -> std::time::Duration {
        std::time::Duration::from_secs(16)
    }
}

#[controller(path = "/writes")]
struct WritesController;

#[routes]
impl WritesController {
    #[post("/")]
    async fn write(&self) -> &'static str {
        "written"
    }

    #[post("/slow")]
    async fn slow(&self) -> &'static str {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        "written"
    }
}

/// The writes, over a binding whose commit is refused.
struct RefusedCommits;

impl nest_rs_core::Discoverable for RefusedCommits {
    fn register(builder: nest_rs_core::ContainerBuilder) -> nest_rs_core::ContainerBuilder {
        builder.provide_dyn::<dyn nest_rs_database::UnitOfWork>(std::sync::Arc::new(Refusing))
    }
}

#[module(providers = [WritesController, RefusedCommits])]
struct RefusedCommitsModule;

#[module(providers = [WritesController])]
struct WritesModule;

/// The one line filed, and the bytes the client read.
async fn line_and_body(
    logs: &LogCapture,
    resp: nest_rs_testing::TestResponse,
) -> (nest_rs_testing::CapturedEvent, usize) {
    let body = resp.0.into_body().into_bytes().await.expect("the body");
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    (line, body.len())
}

/// Every answer the framework renders itself — a path nothing serves, a method
/// its path does not take, a unit its database refused to commit, a handler
/// cut at the request's deadline — is a body the client read, and the line
/// counts it.
#[tokio::test]
async fn an_answer_the_framework_rendered_counts_the_bytes_it_wrote() {
    for (path, method, status) in [
        ("/nowhere", nest_rs_http::Method::GET, 404_u16),
        ("/writes", nest_rs_http::Method::GET, 405),
    ] {
        let logs = LogCapture::install();
        let client = boot::<WritesModule>().await;
        let resp = client.request(method, path).send().await;
        resp.assert_status(nest_rs_http::StatusCode::from_u16(status).expect("a status"));
        let (line, written) = line_and_body(&logs, resp).await;
        assert!(written > 0, "{path}: a problem document was written");
        assert_eq!(
            line.field("bytes"),
            Some(written.to_string()),
            "{path}: {:?}",
            line.fields
        );
    }
}

#[tokio::test]
async fn a_unit_its_database_refused_counts_the_problem_it_answered() {
    let logs = LogCapture::install();
    let client = boot::<RefusedCommitsModule>().await;
    let resp = client.post("/writes").send().await;
    resp.assert_status(nest_rs_http::StatusCode::INTERNAL_SERVER_ERROR);
    let (line, written) = line_and_body(&logs, resp).await;
    assert!(written > 0, "a problem document was written");
    assert_eq!(
        line.field("bytes"),
        Some(written.to_string()),
        "{:?}",
        line.fields
    );
}

#[tokio::test(start_paused = true)]
async fn a_handler_cut_at_its_deadline_counts_the_problem_it_answered() {
    let logs = LogCapture::install();
    let client = crate::boot_on::<WritesModule>(nest_rs_http::HttpConfig {
        request_timeout: Some(std::time::Duration::from_secs(1)),
        ..nest_rs_http::HttpConfig::default()
    })
    .await;
    let resp = client.post("/writes/slow").send().await;
    resp.assert_status(nest_rs_http::StatusCode::SERVICE_UNAVAILABLE);
    let (line, written) = line_and_body(&logs, resp).await;
    assert!(written > 0, "a problem document was written");
    assert_eq!(
        line.field("bytes"),
        Some(written.to_string()),
        "{:?}",
        line.fields
    );
}
