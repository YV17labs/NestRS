//! `GraphqlConfig.max_depth` / `max_complexity` translate into validation
//! limits on every incoming query — too-deep or too-complex queries get
//! rejected during async-graphql's validation pass, before any resolver runs.

use async_graphql::SimpleObject;
use nest_rs_core::{Module, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;

#[derive(SimpleObject)]
struct NestedNode {
    label: String,
}

/// A type nesting as deep as a document asks.
struct Chain;

#[async_graphql::Object]
impl Chain {
    async fn next(&self) -> Chain {
        Chain
    }

    async fn end(&self) -> bool {
        true
    }
}

#[resolver]
struct LimitsResolver;

#[operations]
impl LimitsResolver {
    #[query]
    #[public]
    async fn root(&self) -> NestedNode {
        NestedNode {
            label: "root".into(),
        }
    }

    #[query]
    #[public]
    async fn chain(&self) -> Chain {
        Chain
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

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        max_depth: None,
        max_complexity: None,
        ..Default::default()
    }),
    LimitsFeatureModule,
])]
struct AppNoDepthLimit;

async fn boot<M: Module + 'static>(context: &str) -> TestApp {
    TestApp::builder()
        .module::<M>()
        .build()
        .await
        .unwrap_or_else(|e| panic!("schema boots ({context}): {e:?}"))
}

/// POST `query` and return the body. The tests pin `data: null` rather than
/// async-graphql's error wording, which is not a stable contract.
async fn submit(app: &TestApp, query: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
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

/// A document nesting `depth` fields: `chain`, then `next`s, then `end`.
fn nested(depth: usize) -> String {
    let opened = "next { ".repeat(depth - 2);
    let closed = " }".repeat(depth - 2);
    format!("{{ chain {{ {opened}end{closed} }} }}")
}

/// async-graphql refuses a document past its own recursion bound whatever
/// `max_depth` says, which is why `MAX_DEPTH`'s ceiling is the deepest document
/// that bound admits: past it, a limit limits nothing.
#[tokio::test]
async fn no_depth_limit_reaches_past_the_librarys_recursion_bound() {
    let app = boot::<AppNoDepthLimit>("no depth limit").await;
    let ceiling = nest_rs_graphql::GraphqlConfig::MAX_DEPTH
        .bounds()
        .most()
        .count;
    let ceiling = usize::try_from(ceiling).expect("a depth fits");
    let deepest = submit(&app, &nested(ceiling)).await;
    assert!(
        errors(&deepest).is_empty(),
        "the ceiling itself is a document async-graphql serves, got {deepest}",
    );
    let past = submit(&app, &nested(ceiling + 1)).await;
    assert!(
        past.to_string().contains("recursion depth"),
        "one field deeper is refused with no limit set, got {past}",
    );
}
