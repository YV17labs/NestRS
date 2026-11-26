//! A decorated tool and resource listing of a `#[tools]` block, and its tool,
//! prompt, resource and listing attempted again.

use std::sync::Arc;

use nest_rs_core::module;
use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Resource, Role};
use nest_rs_mcp::{McpAccess, McpError, McpModule, mcp, tools};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::mcp::{call_method, call_tool, open_session, result};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once};

const PATH: &str = "/mcp/decorated";

#[mcp(path = "/mcp/decorated")]
#[derive(Clone)]
struct DecoratedTools {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[tools]
impl DecoratedTools {
    /// Preview a link through the upstream, under its policy.
    #[tool]
    #[public]
    #[resilient(upstream)]
    async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }

    #[resources]
    #[public]
    #[resilient(upstream)]
    async fn parcels(&self) -> Result<Vec<Resource>, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(vec![Resource::new("parcel://7", "parcel 7")])
    }

    /// Where a parcel the listing lists stands.
    #[resource(template = "parcel://{id}")]
    #[public]
    async fn parcel(&self, id: u32) -> Result<String, McpError> {
        Ok(format!("parcel {id} waits in Lyon"))
    }
}

#[module(
    imports = [
        McpModule::for_root(McpAccess::Public),
        ResilienceModule::for_root(opening_on_the_first_failure()),
    ],
    providers = [DecoratedTools, Attempts],
)]
struct DecoratedMcpModule;

#[tokio::test(start_paused = true)]
async fn a_tool_answers_its_refusal_unavailable_with_the_breakers_wait() {
    let app = TestApp::for_module::<DecoratedMcpModule>()
        .await
        .expect("a decorated #[mcp] host boots");

    let failed = call_tool(app.http(), PATH, "decorated", None).await;
    assert!(failed.contains("\"code\":\"INTERNAL\""), "{failed}");

    let refused = call_tool(app.http(), PATH, "decorated", None).await;
    assert!(refused.contains("\"code\":\"UNAVAILABLE\""), "{refused}");
    assert!(refused.contains("\"retryAfterSeconds\":30"), "{refused}");
}

#[tokio::test(start_paused = true)]
async fn a_resource_listing_answers_its_refusal_unavailable_with_the_breakers_wait() {
    let app = TestApp::for_module::<DecoratedMcpModule>()
        .await
        .expect("a decorated listing boots");
    let session = open_session(app.http(), PATH, None).await;

    let failed = call_method(
        app.http(),
        PATH,
        &session,
        None,
        "resources/list",
        json!({}),
    )
    .await;
    assert!(failed.contains("\"code\":\"INTERNAL\""), "{failed}");

    let refused = call_method(
        app.http(),
        PATH,
        &session,
        None,
        "resources/list",
        json!({}),
    )
    .await;
    assert!(refused.contains("\"code\":\"UNAVAILABLE\""), "{refused}");
    assert!(refused.contains("\"retryAfterSeconds\":30"), "{refused}");
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(
        attempts.made(),
        1,
        "sent once, and the listing the breaker refused never"
    );
}

const RETRIED: &str = "/mcp/retried";

#[mcp(path = "/mcp/retried")]
#[derive(Clone)]
struct RetriedTools {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[tools]
impl RetriedTools {
    /// Track a parcel through the carrier, under its policy.
    #[tool]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn track(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok("in transit".to_owned())
    }

    /// Draft a note to the carrier, under its policy.
    #[prompt]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn draft(&self) -> Result<GetPromptResult, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "drafted",
        )]))
    }

    /// Where a parcel stands, read from the carrier under its policy.
    #[resource(template = "parcel://{id}")]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn parcel(&self, id: u32) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(format!("parcel {id} waits in Lyon"))
    }

    #[resources]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn parcels(&self) -> Result<Vec<Resource>, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(vec![Resource::new("parcel://7", "parcel 7")])
    }
}

#[module(
    imports = [
        McpModule::for_root(McpAccess::Public),
        ResilienceModule::for_root(retrying_once()),
    ],
    providers = [RetriedTools, Attempts],
)]
struct RetriedMcpModule;

/// What the host answers `method` with on a session of its own, and how
/// many attempts the one decorated method it reached made.
async fn retried(method: &str, params: Value) -> (Value, u32) {
    let app = TestApp::for_module::<RetriedMcpModule>()
        .await
        .expect("a host decorating its prompt, its resource and its listing boots");
    let session = open_session(app.http(), RETRIED, None).await;
    let body = call_method(app.http(), RETRIED, &session, None, method, params).await;
    let attempts = app.container().get::<Attempts>().expect("provided");
    (result(&body), attempts.made())
}

#[tokio::test(start_paused = true)]
async fn a_tool_failing_once_answers_on_its_retry_within_its_one_call() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<RetriedMcpModule>()
        .await
        .expect("a host decorating its tool boots");

    let called = call_tool(app.http(), RETRIED, "track", None).await;
    let answer = result(&called);
    assert_eq!(
        answer["result"]["content"][0]["text"], "in transit",
        "{answer}"
    );
    assert_ne!(answer["result"]["isError"], true, "{answer}");

    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    retried_once(&logs, "RetriedTools", "track");
    let mut calls = logs.find(OPERATIONS, nest_rs_mcp::unit::OPERATION.name());
    calls.retain(|line| line.field("tool").as_deref() == Some("track"));
    assert_eq!(calls.len(), 1, "one call of the tool: {calls:#?}");
    assert_eq!(calls[0].field("outcome").as_deref(), Some(OK));
}

#[tokio::test(start_paused = true)]
async fn a_prompt_failing_once_answers_on_its_retry() {
    let (answer, attempts) =
        retried("prompts/get", json!({ "name": "draft", "arguments": {} })).await;
    assert_eq!(
        answer["result"]["messages"][0]["content"]["text"], "drafted",
        "{answer}"
    );
    assert_eq!(attempts, 2, "the failed attempt and its retry");
}

#[tokio::test(start_paused = true)]
async fn a_resource_failing_once_answers_on_its_retry() {
    let (answer, attempts) = retried("resources/read", json!({ "uri": "parcel://7" })).await;
    assert_eq!(
        answer["result"]["contents"][0]["text"], "parcel 7 waits in Lyon",
        "{answer}"
    );
    assert_eq!(attempts, 2, "the failed attempt and its retry");
}

#[tokio::test(start_paused = true)]
async fn a_resource_listing_failing_once_answers_on_its_retry() {
    let (answer, attempts) = retried("resources/list", json!({})).await;
    assert_eq!(
        answer["result"]["resources"][0]["uri"], "parcel://7",
        "{answer}"
    );
    assert_eq!(attempts, 2, "the failed attempt and its retry");
}
