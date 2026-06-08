//! `GraphqlConfig.max_depth` / `max_complexity` translate into validation
//! limits on every incoming query — too-deep or too-complex queries get
//! rejected during async-graphql's validation pass, before any resolver runs.

use async_graphql::SimpleObject;
use nest_rs_core::{Module, module};
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

/// Boot the three test apps through the same code path so a future change to
/// the boot recipe lands in one place instead of three.
async fn boot<M: Module + 'static>(context: &str) -> TestApp {
    TestApp::builder()
        .module::<M>()
        .http(HttpTransport::new())
        .build()
        .await
        .unwrap_or_else(|e| panic!("schema boots ({context}): {e:?}"))
}

/// POST `query` and pull the GraphQL `errors[]` count + the rejection's data
/// shape. Asserts `data` is `null` on rejection so the test pins behaviour
/// (rejection happens before resolver execution) instead of the exact
/// async-graphql error wording — that wording is not a stable contract.
async fn submit(app: &TestApp, query: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    // async-graphql returns validation failures as 200 with `errors[]`.
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize::<serde_json::Value>()
}

fn errors(body: &serde_json::Value) -> &[serde_json::Value] {
    body.get("errors")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

#[tokio::test]
async fn depth_limit_rejects_too_deep_query() {
    let app = boot::<AppDepthLimited>("max_depth = 1").await;
    // depth 2 (`root` → `label`) exceeds max_depth = 1.
    let body = submit(&app, "{ root { label } }").await;
    assert!(
        !errors(&body).is_empty(),
        "expected at least one validation error for over-depth query, body = {body}",
    );
    assert!(
        body.get("data").is_some_and(serde_json::Value::is_null),
        "data must be null when validation rejects (rejection runs before resolution)",
    );
}

#[tokio::test]
async fn complexity_limit_rejects_too_complex_query() {
    let app = boot::<AppComplexityLimited>("max_complexity = 1").await;
    // Complexity 2 (`root` + `label`, default 1 per field) exceeds 1.
    let body = submit(&app, "{ root { label } }").await;
    assert!(
        !errors(&body).is_empty(),
        "expected at least one validation error for over-complex query, body = {body}",
    );
    assert!(
        body.get("data").is_some_and(serde_json::Value::is_null),
        "data must be null when validation rejects",
    );
}

#[tokio::test]
async fn within_limits_still_resolves() {
    let app = boot::<AppGenerousLimits>("generous limits").await;
    let body = submit(&app, "{ root { label } }").await;
    assert!(
        errors(&body).is_empty(),
        "generous limits should not reject, got errors = {:?}",
        errors(&body),
    );
    let label = body
        .pointer("/data/root/label")
        .and_then(|v| v.as_str())
        .expect("data.root.label present");
    assert_eq!(label, "root");
}
