//! Mirror tests for `src/graphql/` — only compiled when the `graphql` feature is on.

use nest_rs_testing::TestApp;

mod authorize;
mod mask;

/// POST one operation as `role` (empty ⇒ no header, i.e. the anonymous caller)
/// and return the response body as plain JSON. Both modules drive `/graphql`
/// the same way, so the driver lives here once instead of per test.
pub(crate) async fn query(app: &TestApp, role: &str, query: &str) -> serde_json::Value {
    let mut req = app.http().post("/graphql");
    if !role.is_empty() {
        req = req.header("x-role", role);
    }
    let resp = req
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    serde_json::to_value(resp.json().await).expect("a GraphQL response is JSON")
}
