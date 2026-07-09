use std::sync::Arc;

use nest_rs_mcp::{
    McpOperationGuard, ServerHandler, endpoint_with_guard, tool_handler, tool_router,
};
use poem::http::StatusCode;
use poem::test::TestClient;
use poem::{Error, Request, Response};

struct RejectGuard;

impl McpOperationGuard for RejectGuard {
    fn before<'a>(&'a self, _req: &'a mut Request) -> nest_rs_mcp::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move {
            Err(Error::from_response(
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body("nope"),
            ))
        })
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
async fn endpoint_without_an_explicit_guard_is_denied_by_default() {
    let open = nest_rs_mcp::endpoint(|| DummyHandler);
    let resp = TestClient::new(open).post("/").send().await;
    assert_eq!(resp.0.status(), StatusCode::UNAUTHORIZED);
}

// The `#[mcp]` macro resolves its guard via `get_dyn` and forwards the
// `Option` straight to `endpoint_with_guard` — so `None` (no guard wired in
// the container) MUST fail closed, not serve the tool surface open.
#[tokio::test]
async fn endpoint_with_guard_none_falls_back_to_deny_all() {
    let unwired = endpoint_with_guard(None, || DummyHandler);
    let resp = TestClient::new(unwired).post("/").send().await;
    assert_eq!(resp.0.status(), StatusCode::UNAUTHORIZED);
}

// The explicit opt-in counterpart to deny-all: wiring `AllowAllMcpGuard`
// admits the request so a deliberately public tool can be served.
#[tokio::test]
async fn allow_all_guard_admits_the_request() {
    let guard = nest_rs_mcp::AllowAllMcpGuard;
    let mut req = Request::default();
    assert!(guard.before(&mut req).await.is_ok());
}
