//! `#[injectable(scope = request)]`: fresh instance per request, cached for
//! the life of that request, resolved with `Scoped<T>`, the one type every edge
//! takes as a parameter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use nest_rs_core::{Scoped, injectable, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{controller, routes};
use nest_rs_mcp::{DEFAULT_PATH, McpAccess, McpError, McpModule, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_tool, result};
use nest_rs_ws::{WsModule, gateway, messages};

#[injectable]
#[derive(Default)]
struct Sequence {
    next: AtomicU64,
}

impl Sequence {
    fn take(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[injectable(scope = request)]
struct RequestId {
    #[inject]
    seq: Arc<Sequence>,
    id: OnceLock<u64>,
}

impl RequestId {
    fn id(&self) -> u64 {
        *self.id.get_or_init(|| self.seq.take())
    }
}

#[controller(path = "/")]
struct ScopeController;

#[routes]
impl ScopeController {
    #[get("/id")]
    #[public]
    async fn id(&self, a: Scoped<RequestId>, b: Scoped<RequestId>) -> String {
        format!("{}-{}-{}", a.id(), b.id(), Arc::ptr_eq(&a.0, &b.0))
    }
}

#[module(providers = [Sequence, RequestId, ScopeController])]
struct ScopeModule;

#[tokio::test]
async fn instance_is_cached_within_a_request_and_fresh_across_requests() {
    let app = TestApp::for_module::<ScopeModule>().await.expect("boots");

    let first = app.http().get("/id").send().await;
    first.assert_status_is_ok();
    first.assert_text("1-1-true").await;

    let second = app.http().get("/id").send().await;
    second.assert_status_is_ok();
    second.assert_text("2-2-true").await;
}

#[resolver]
struct ScopeResolver;

#[operations]
impl ScopeResolver {
    #[query]
    #[public]
    async fn request_id(&self, id: Scoped<RequestId>) -> u64 {
        id.id()
    }
}

#[gateway(path = "/scope")]
struct ScopeGateway;

#[messages]
impl ScopeGateway {
    #[subscribe_message("id")]
    #[public]
    async fn id(&self, id: Scoped<RequestId>) -> u64 {
        id.id()
    }
}

#[mcp]
struct ScopeHost;

#[tools]
impl ScopeHost {
    #[tool(description = "The request's id.")]
    #[public]
    async fn request_id(&self, id: Scoped<RequestId>) -> Result<String, McpError> {
        Ok(id.id().to_string())
    }
}

#[module(
    imports = [GraphqlModule, WsModule, McpModule::for_root(McpAccess::Public)],
    providers = [Sequence, RequestId, ScopeController, ScopeResolver, ScopeGateway, ScopeHost],
)]
struct EveryEdgeModule;

/// One provider, read through one type, on all four edges: the handler's import
/// names no edge, so a feature with several adapters writes one `use`.
#[tokio::test]
async fn one_scoped_type_resolves_a_provider_on_every_edge() {
    let app = TestApp::for_module::<EveryEdgeModule>()
        .await
        .expect("boots");

    app.http()
        .get("/id")
        .send()
        .await
        .assert_text("1-1-true")
        .await;

    let query = serde_json::json!({ "query": "{ requestId }" });
    let graphql = app.http().post("/graphql").body_json(&query).send().await;
    graphql
        .assert_json(serde_json::json!({ "data": { "requestId": 2 } }))
        .await;

    let tool = call_tool(app.http(), DEFAULT_PATH, "request_id", None).await;
    assert_eq!(result(&tool)["result"]["content"][0]["text"], "3");

    let sockets = TestApp::builder()
        .module::<EveryEdgeModule>()
        .build_ws()
        .await
        .expect("boots");
    let mut socket = sockets.socket("/scope").connect().await;
    socket.send("id", serde_json::Value::Null).await;
    assert_eq!(socket.next_envelope().await["data"], 1);
    sockets.shutdown().await.expect("shuts down");
}
