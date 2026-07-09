//! Global pipes on GraphQL operation **variables**. A registered `GlobalPipe`'s
//! `transform_graphql_variables` runs over an operation's variables before
//! execution — the operation-level analog of HTTP's `transform_body`, wired at
//! the `/graphql` endpoint via the `GraphqlVariablePipe` bridge that
//! `use_pipes_global` seeds. A rejection becomes a GraphQL error.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::{GraphqlModule, resolver};
use nest_rs_guards::pipe;
use nest_rs_http::HttpTransport;
use nest_rs_pipes::{GlobalPipe, PipeError};
use nest_rs_testing::TestApp;
use serde_json::Value;

/// Uppercases the `raw` string variable, and rejects the literal `"boom"` —
/// exercises both the transform and the error path over variables.
#[injectable]
#[derive(Default)]
struct VarPipe;

impl Layer for VarPipe {}

impl GlobalPipe for VarPipe {
    fn transform_graphql_variables(&self, value: &mut Value) -> Result<(), PipeError> {
        if let Some(raw) = value.get("raw").and_then(Value::as_str) {
            if raw == "boom" {
                return Err(PipeError::new("boom is not allowed"));
            }
            let upper = raw.to_uppercase();
            value["raw"] = Value::String(upper);
        }
        Ok(())
    }
}

#[resolver]
struct EchoResolver;

#[resolver]
impl EchoResolver {
    #[query]
    #[public]
    async fn echo(&self, raw: String) -> String {
        raw
    }
}

#[module(providers = [EchoResolver, VarPipe])]
struct EchoFeatureModule;

#[module(imports = [GraphqlModule::for_root(None), EchoFeatureModule])]
struct AppWithVarPipe;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AppWithVarPipe>()
        .use_pipes_global([pipe::<VarPipe>()])
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn a_global_pipe_transforms_operation_variables_before_execution() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "hi" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    // The resolver saw the pipe-transformed variable, not the raw one.
    let echo = json
        .value()
        .object()
        .get("data")
        .object()
        .get("echo")
        .string();
    assert_eq!(echo, "HI");
}

#[tokio::test]
async fn a_rejecting_variable_pipe_surfaces_a_graphql_error() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "boom" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors
        .iter()
        .next()
        .expect("a variable-pipe rejection yields one error");
    assert_eq!(
        first.object().get("message").string(),
        "boom is not allowed"
    );
}
