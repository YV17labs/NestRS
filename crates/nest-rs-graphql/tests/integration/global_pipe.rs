//! Global pipes on GraphQL operation **variables**. A registered `GlobalPipe`'s
//! `transform_graphql_variables` runs over an operation's variables before
//! execution — the operation-level analog of HTTP's `transform_body`, wired at
//! the `/graphql` endpoint via the `GraphqlVariablePipe` bridge that
//! `use_pipes_global` seeds. A rejection becomes a GraphQL error.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::pipe;
use nest_rs_http::HttpTransport;
use nest_rs_pipes::{GlobalPipe, PipeError};
use nest_rs_testing::TestApp;
use serde_json::Value;

/// Uppercases the `raw` string variable, rejects the literal `"boom"`, and
/// flattens the variables into a string for `"flatten"` — exercises the
/// transform, the pipe's own error, and a pipe that breaks the variables' shape.
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
            if raw == "flatten" {
                *value = Value::String(FLATTENED.to_owned());
                return Ok(());
            }
            let upper = raw.to_uppercase();
            value["raw"] = Value::String(upper);
        }
        Ok(())
    }
}

/// What the pipe flattens the variables into, which no error may quote.
const FLATTENED: &str = "sk_live_51HsecretTOKEN";

#[resolver]
struct EchoResolver;

#[operations]
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

/// A pipe that leaves the variables in a shape GraphQL cannot take is reported
/// by where and what kind — never by the value, which is the caller's.
#[tokio::test]
async fn variables_a_pipe_left_unusable_are_reported_without_their_value() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "flatten" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors.iter().next().expect("one error");
    let message = first.object().get("message").string().to_owned();
    assert!(!message.contains(FLATTENED), "{message}");
    assert_eq!(
        message,
        "variable pipe produced an invalid variables object: invalid type: a string, expected a map"
    );
}
