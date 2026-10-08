//! A GraphQL operation proven read-only runs **outside** the request
//! transaction every POST is handed; a mutation keeps it.

use std::any::Any;
use std::sync::Arc;

use nest_rs_core::{Layer, module};
use nest_rs_database::{Executor, with_request_executor};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::async_trait;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use nest_rs_testing::TestApp;
use poem::{Request, Response, Result};

/// A stand-in for the ORM's executor: `"txn"` yields a `"pool"` sibling.
struct MarkerExecutor {
    kind: &'static str,
}

impl Executor for MarkerExecutor {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn non_transactional(&self) -> Option<Arc<dyn Executor>> {
        match self.kind {
            "txn" => Some(Arc::new(MarkerExecutor { kind: "pool" })),
            _ => None,
        }
    }
}

/// The kind of executor ambient right now — `"none"` outside any scope.
fn ambient_kind() -> String {
    let Some(executor) = nest_rs_database::current_executor() else {
        return "none".into();
    };
    match executor.as_any().downcast_ref::<MarkerExecutor>() {
        Some(marker) => marker.kind.into(),
        None => "unknown".into(),
    }
}

/// Installs the transactional handle around every request, like `DbContext`.
#[interceptor(priority = -10)]
struct MarkerDbContext;

impl Layer for MarkerDbContext {}

#[async_trait]
impl Interceptor for MarkerDbContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        with_request_executor(Arc::new(MarkerExecutor { kind: "txn" }), next.run(req)).await
    }
}

#[resolver]
struct ExecutorResolver;

#[operations]
impl ExecutorResolver {
    #[query]
    #[public]
    async fn ambient_executor(&self) -> String {
        ambient_kind()
    }

    #[mutation]
    #[public]
    async fn write_ambient_executor(&self) -> String {
        ambient_kind()
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [MarkerDbContext, ExecutorResolver]
)]
struct ReadOnlyTestModule;

async fn ambient_executor_for(query: &str, field: &str) -> String {
    let app = TestApp::builder()
        .module::<ReadOnlyTestModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();

    resp.json()
        .await
        .value()
        .object()
        .get("data")
        .object()
        .get(field)
        .string()
        .to_owned()
}

#[tokio::test]
async fn a_query_runs_outside_the_request_transaction() {
    assert_eq!(
        ambient_executor_for("{ ambientExecutor }", "ambientExecutor").await,
        "pool",
        "a read-only operation must not pay for the POST's transaction",
    );
}

#[tokio::test]
async fn a_mutation_keeps_the_request_transaction() {
    assert_eq!(
        ambient_executor_for("mutation { writeAmbientExecutor }", "writeAmbientExecutor",).await,
        "txn",
    );
}

#[tokio::test]
async fn a_batch_holding_a_mutation_keeps_the_transaction_for_the_query_too() {
    // Per batch: one executor is installed for the whole request.
    let app = TestApp::builder()
        .module::<ReadOnlyTestModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!([
            { "query": "{ ambientExecutor }" },
            { "query": "mutation { writeAmbientExecutor }" },
        ]))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let entries = json.value().array();
    assert_eq!(
        entries
            .get(0)
            .object()
            .get("data")
            .object()
            .get("ambientExecutor")
            .string(),
        "txn",
    );
}
