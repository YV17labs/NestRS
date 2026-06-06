use std::sync::Arc;

use nest_rs_mcp::{
    McpOperationGuard, ServerHandler, endpoint_with_guard, tool_handler, tool_router,
};
use nest_rs_middleware::Guard;
use poem::http::StatusCode;
use poem::test::TestClient;
use poem::{Error, Request, Response};

struct RejectAll;

#[async_trait::async_trait]
impl Guard for RejectAll {
    async fn check(&self, _req: &mut Request) -> Result<(), Response> {
        Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body("nope"))
    }
}

struct RejectGuard;

impl McpOperationGuard for RejectGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> nest_rs_mcp::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move { RejectAll.check(req).await.map_err(Error::from_response) })
    }
}

#[derive(Clone)]
struct DummyHandler;

#[tool_router]
impl DummyHandler {}

#[tool_handler]
impl ServerHandler for DummyHandler {}

#[tokio::test]
async fn endpoint_with_guard_rejects_before_the_handler_runs() {
    let guarded = endpoint_with_guard(
        Some(Arc::new(RejectGuard) as Arc<dyn McpOperationGuard>),
        || DummyHandler,
    );
    let resp = TestClient::new(guarded).post("/").send().await;
    assert_eq!(resp.0.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn endpoint_without_a_guard_accepts_requests() {
    let open = endpoint_with_guard(None, || DummyHandler);
    let resp = TestClient::new(open).post("/").send().await;
    assert_ne!(resp.0.status(), StatusCode::UNAUTHORIZED);
}
