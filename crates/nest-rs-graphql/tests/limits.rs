//! `GraphqlConfig.max_depth` / `max_complexity` translate into validation
//! limits on every incoming query — too-deep or too-complex queries get
//! rejected during async-graphql's validation pass, before any resolver runs.

use async_graphql::SimpleObject;
use nest_rs_core::module;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_testing::TestApp;

#[derive(SimpleObject)]
struct NestedNode {
    label: String,
}

#[resolver]
struct LimitsResolver;

#[resolver]
impl LimitsResolver {
    #[query]
    async fn root(&self) -> NestedNode {
        NestedNode {
            label: "root".into(),
        }
    }
}

#[module(providers = [LimitsResolver])]
struct LimitsFeatureModule;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_depth: Some(1),
        ..Default::default()
    }),
    LimitsFeatureModule,
])]
struct AppDepthLimited;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_complexity: Some(1),
        ..Default::default()
    }),
    LimitsFeatureModule,
])]
struct AppComplexityLimited;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_depth: Some(10),
        max_complexity: Some(100),
        ..Default::default()
    }),
    LimitsFeatureModule,
])]
struct AppGenerousLimits;

#[tokio::test]
async fn depth_limit_rejects_too_deep_query() {
    let app = TestApp::builder()
        .module::<AppDepthLimited>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("schema boots with max_depth = 1");

    // depth 2 (`root` → `label`) exceeds max_depth = 1.
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ root { label } }" }))
        .send()
        .await;
    // async-graphql returns validation failures as 200 with `errors[]`.
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    assert!(
        errors.len() >= 1,
        "expected at least one validation error for over-depth query"
    );
    let msg = errors
        .iter()
        .next()
        .unwrap()
        .object()
        .get("message")
        .string()
        .to_lowercase();
    assert!(
        msg.contains("deep") || msg.contains("depth"),
        "depth-limit error should mention depth, got: {msg}"
    );
}

#[tokio::test]
async fn complexity_limit_rejects_too_complex_query() {
    let app = TestApp::builder()
        .module::<AppComplexityLimited>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("schema boots with max_complexity = 1");

    // complexity 2 (`root` + `label`, default 1 per field) exceeds 1.
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ root { label } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    assert!(
        errors.len() >= 1,
        "expected at least one validation error for over-complex query"
    );
    let msg = errors
        .iter()
        .next()
        .unwrap()
        .object()
        .get("message")
        .string()
        .to_lowercase();
    assert!(
        msg.contains("complex"),
        "complexity-limit error should mention complexity, got: {msg}"
    );
}

#[tokio::test]
async fn within_limits_still_resolves() {
    let app = TestApp::builder()
        .module::<AppGenerousLimits>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("schema boots with generous limits");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ root { label } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let label = json
        .value()
        .object()
        .get("data")
        .object()
        .get("root")
        .object()
        .get("label")
        .string();
    assert_eq!(label, "root");
}
