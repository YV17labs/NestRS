//! Covers `src/error.rs` through the HTTP edge: a client of a route whose
//! dependency's breaker is open is answered `503` with the `Retry-After` the
//! breaker knows, at once — whether the route answers the policy's error or a
//! domain error of its own handing its problem on — and a failed call is
//! answered opaquely.

use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use nest_rs_core::{Problem, ToProblem, code, module};
use nest_rs_http::{controller, routes};
use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::TestApp;

/// What the dependency answers: a failure the route cannot describe.
#[derive(Debug, thiserror::Error)]
#[error("the upstream answered 502")]
struct UpstreamDown;

impl ToProblem for UpstreamDown {
    fn to_problem(&self) -> Option<Problem> {
        None
    }
}

/// A route's own error, wrapping the policy's and answering what it answers.
#[derive(Debug, thiserror::Error)]
#[error("no preview")]
struct NoPreview(#[source] ResilienceError<UpstreamDown>);

impl ToProblem for NoPreview {
    fn to_problem(&self) -> Option<Problem> {
        self.0.to_problem()
    }
}

#[controller(path = "/previews")]
struct PreviewsController {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[routes]
impl PreviewsController {
    #[get("/direct")]
    #[public]
    async fn direct(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        self.upstream
            .run(|_| async { Err::<String, _>(UpstreamDown) })
            .await
    }

    #[get("/wrapped")]
    #[public]
    async fn wrapped(&self) -> Result<String, NoPreview> {
        self.upstream
            .run(|_| async { Err::<String, _>(UpstreamDown) })
            .await
            .map_err(NoPreview)
    }
}

#[module(
    imports = [ResilienceModule::for_root(ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    })],
    providers = [PreviewsController],
)]
struct PreviewsModule;

#[tokio::test]
async fn an_open_breaker_is_answered_503_with_its_retry_after_and_a_failure_opaquely() {
    let app = TestApp::for_module::<PreviewsModule>()
        .await
        .expect("the documented wiring boots");

    let failed = app.http().get("/previews/direct").send().await;
    failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    failed.assert_header_is_not_exist("retry-after");
    let body = failed.json().await;
    body.value()
        .object()
        .get("code")
        .assert_string(code::INTERNAL.as_str());

    for route in ["/previews/direct", "/previews/wrapped"] {
        let refused = app.http().get(route).send().await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        refused.assert_header("retry-after", "30");
        let body = refused.json().await;
        body.value()
            .object()
            .get("code")
            .assert_string(code::UNAVAILABLE.as_str());
    }
}

/// A policy refusing every call but its first, which fails: what every edge
/// below meets on its second call.
fn opening_on_the_first_failure() -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    }
}

mod graphql {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy};
    use nest_rs_testing::TestApp;

    use super::{UpstreamDown, opening_on_the_first_failure};

    #[resolver]
    struct PreviewResolver {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[operations]
    impl PreviewResolver {
        #[query]
        #[public]
        async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            self.upstream
                .run(|_| async { Err::<String, _>(UpstreamDown) })
                .await
        }
    }

    #[module(
        imports = [GraphqlModule, ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [PreviewResolver],
    )]
    struct PreviewGraphqlModule;

    #[tokio::test]
    async fn an_open_breaker_is_a_field_error_saying_unavailable_and_its_wait() {
        let app = TestApp::for_module::<PreviewGraphqlModule>()
            .await
            .expect("a resolver over a policy boots");
        let query = || async {
            app.http()
                .post("/graphql")
                .body_json(&serde_json::json!({ "query": "{ preview }" }))
                .send()
                .await
                .json()
                .await
                .value()
                .deserialize::<serde_json::Value>()
        };
        let failed = query().await;
        assert_eq!(
            failed["errors"][0]["extensions"]["code"], "INTERNAL",
            "{failed}"
        );

        let refused = query().await;
        let extensions = &refused["errors"][0]["extensions"];
        assert_eq!(extensions["code"], "UNAVAILABLE", "{refused}");
        assert_eq!(extensions["retryAfterSeconds"], 30, "{refused}");
    }
}

mod ws {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy};
    use nest_rs_testing::TestApp;
    use nest_rs_ws::{WsModule, gateway, messages};

    use super::{UpstreamDown, opening_on_the_first_failure};

    #[gateway(path = "/ws/previews")]
    struct PreviewGateway {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[messages]
    impl PreviewGateway {
        #[subscribe_message("preview")]
        #[public]
        async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            self.upstream
                .run(|_| async { Err::<String, _>(UpstreamDown) })
                .await
        }
    }

    #[module(
        imports = [WsModule, ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [PreviewGateway],
    )]
    struct PreviewWsModule;

    #[tokio::test]
    async fn an_open_breaker_is_an_error_frame_saying_unavailable_and_its_wait() {
        let app = TestApp::builder()
            .module::<PreviewWsModule>()
            .build_ws()
            .await
            .expect("a gateway over a policy boots on a real port");
        let mut socket = app.socket("/ws/previews").connect().await;

        socket.send("preview", serde_json::Value::Null).await;
        let failed = socket.next_envelope().await;
        assert_eq!(failed["data"]["code"], "INTERNAL", "{failed}");

        socket.send("preview", serde_json::Value::Null).await;
        let refused = socket.next_envelope().await;
        assert_eq!(refused["data"]["code"], "UNAVAILABLE", "{refused}");
        assert_eq!(refused["data"]["retryAfterSeconds"], 30, "{refused}");

        app.shutdown().await.expect("the transport stops cleanly");
    }
}

mod mcp {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_mcp::{McpAccess, McpModule, mcp, tools};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy};
    use nest_rs_testing::TestApp;
    use nest_rs_testing::mcp::call_tool;

    use super::{UpstreamDown, opening_on_the_first_failure};

    const PATH: &str = "/mcp/previews";

    #[mcp(path = "/mcp/previews")]
    #[derive(Clone)]
    struct PreviewTools {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[tools]
    impl PreviewTools {
        /// Preview a link through the upstream.
        #[tool]
        #[public]
        async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            self.upstream
                .run(|_| async { Err::<String, _>(UpstreamDown) })
                .await
        }
    }

    #[module(
        imports = [
            McpModule::for_root(McpAccess::Public),
            ResilienceModule::for_root(opening_on_the_first_failure()),
        ],
        providers = [PreviewTools],
    )]
    struct PreviewMcpModule;

    #[tokio::test]
    async fn an_open_breaker_is_an_error_saying_unavailable_and_its_wait() {
        let app = TestApp::for_module::<PreviewMcpModule>()
            .await
            .expect("an #[mcp] host over a policy boots");

        let failed = call_tool(app.http(), PATH, "preview", None).await;
        assert!(failed.contains("\"code\":\"INTERNAL\""), "{failed}");

        let refused = call_tool(app.http(), PATH, "preview", None).await;
        assert!(refused.contains("\"code\":\"UNAVAILABLE\""), "{refused}");
        assert!(refused.contains("\"retryAfterSeconds\":30"), "{refused}");
    }
}

/// A job failing on an open breaker waits at least the breaker's `Retry-After`
/// before its next attempt, through the problem its error's chain carries, and
/// one refused by a spent rate the rate's, up to an hour.
mod queue {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::Container;
    use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
    use nest_rs_queue::{
        Capabilities, ProcessMethod, QueueBackend, QueueName, WIRE_FORMAT_VERSION, processor, queue,
    };
    use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResiliencePolicy};
    use serde::{Deserialize, Serialize};

    use super::{UpstreamDown, opening_on_the_first_failure};

    static BARE: QueueBackend = QueueBackend::new("bare", Capabilities::NONE);

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct PreviewCommand {
        url: String,
    }

    #[queue(name = "previews", job = PreviewCommand)]
    struct PreviewsQueue;

    #[nest_rs_core::injectable]
    struct PreviewProcessor {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[processor]
    impl PreviewProcessor {
        #[process(queue = PreviewsQueue, retries = 3)]
        async fn preview(&self, job: PreviewCommand) -> Result<(), ResilienceError<UpstreamDown>> {
            let _ = job;
            self.upstream.run(|_| async { Err(UpstreamDown) }).await
        }
    }

    async fn attempted(container: &Container) -> AttemptOutcome {
        let method = nest_rs_core::inventory::iter::<ProcessMethod>()
            .find(|method| method.name() == "PreviewProcessor::preview")
            .expect("the method is submitted");
        let mut delivery = Delivery::new(
            &BARE,
            QueueName::new("previews").expect("a valid name"),
            serde_json::json!({
                "v": WIRE_FORMAT_VERSION,
                "payload": { "url": "https://example.com" },
            }),
        );
        consume::attempt(method, &mut delivery, container.clone(), None).await
    }

    #[tokio::test]
    async fn a_job_failing_on_an_open_breaker_waits_at_least_its_retry_after() {
        let (_app, upstream) = crate::policy(opening_on_the_first_failure()).await;
        let container = Container::builder()
            .provide(PreviewProcessor { upstream })
            .build()
            .expect("the container builds");

        let failed = attempted(&container).await;
        assert!(matches!(failed, AttemptOutcome::Retry { .. }), "{failed:?}");

        let AttemptOutcome::Retry { after } = attempted(&container).await else {
            panic!("a 503 another attempt may clear retries");
        };
        let asked = Duration::from_secs(30);
        assert!(
            after >= asked && after <= asked.mul_f64(1.2),
            "the breaker's wait, stretched by a fifth at most: {after:?}"
        );
    }

    #[tokio::test]
    async fn a_job_refused_by_a_spent_rate_waits_its_retry_after_up_to_an_hour() {
        let window = Duration::from_secs(6 * 60 * 60);
        let (_app, upstream) = crate::policy(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: window,
            ..crate::quiet()
        })
        .await;
        let container = Container::builder()
            .provide(PreviewProcessor { upstream })
            .build()
            .expect("the container builds");

        let sent = attempted(&container).await;
        assert!(
            matches!(sent, AttemptOutcome::Retry { .. }),
            "the window's one attempt, failed: {sent:?}"
        );

        let AttemptOutcome::Retry { after } = attempted(&container).await else {
            panic!("a 503 another attempt may clear retries");
        };
        let hour = Duration::from_secs(60 * 60);
        assert!(
            after >= hour && after <= hour.mul_f64(1.2),
            "an hour of the {window:?} the rate told, stretched by a fifth at most: {after:?}"
        );
    }
}
