//! Per-argument pipes on GraphQL operations. A `Piped<P, T>` / `Valid<T>`
//! parameter exposes the wire value type `T`, runs the pipe before the body,
//! and surfaces a rejection as a GraphQL error — the resolver body only ever
//! unwraps the already-transformed value. The GraphQL analog of the HTTP
//! `Piped<P, E>` / `Valid<E>` extractors.

use async_graphql::InputObject;
use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_pipes::{Pipe, PipeError, Piped, Trim, Valid};
use nest_rs_testing::TestApp;
use validator::Validate;

/// A pipe that always rejects — exercises the error path.
struct Reject;

impl Pipe for Reject {
    type In = String;
    type Out = String;
    fn transform(_: String) -> Result<String, PipeError> {
        Err(PipeError::new("bad input"))
    }
}

#[derive(InputObject, Validate)]
struct NameInput {
    #[validate(length(min = 1))]
    name: String,
}

#[resolver]
struct PipeResolver;

#[resolver]
impl PipeResolver {
    /// `Piped<Trim, String>`: the SDL arg is `String`; the body sees it trimmed.
    #[query]
    #[public]
    async fn trimmed(&self, raw: Piped<Trim, String>) -> async_graphql::Result<String> {
        Ok(raw.into_inner())
    }

    /// A rejecting pipe surfaces as a GraphQL error, never reaching the body.
    #[query]
    #[public]
    async fn checked(&self, raw: Piped<Reject, String>) -> async_graphql::Result<String> {
        Ok(raw.into_inner())
    }

    /// `Valid<T>`: validates the input object, exposing `NameInput` on the wire.
    #[query]
    #[public]
    async fn named(&self, input: Valid<NameInput>) -> async_graphql::Result<String> {
        Ok(input.into_inner().name)
    }
}

#[module(providers = [PipeResolver])]
struct PipeFeatureModule;

#[module(imports = [GraphqlModule::for_root(None), PipeFeatureModule])]
struct AppWithPipes;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AppWithPipes>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn a_piped_arg_runs_the_pipe_before_the_body() {
    // The wire arg is `String` (the query would fail schema validation if it were
    // the `Piped` wrapper), and the body receives the trimmed value.
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ trimmed(raw: \"  hi  \") }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let trimmed = json
        .value()
        .object()
        .get("data")
        .object()
        .get("trimmed")
        .string();
    assert_eq!(trimmed, "hi");
}

#[tokio::test]
async fn a_rejecting_pipe_surfaces_a_graphql_error() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ checked(raw: \"whatever\") }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors
        .iter()
        .next()
        .expect("a pipe rejection yields one error");
    assert_eq!(first.object().get("message").string(), "bad input");
}

#[tokio::test]
async fn a_valid_arg_accepts_a_valid_input() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ named(input: { name: \"ok\" }) }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let named = json
        .value()
        .object()
        .get("data")
        .object()
        .get("named")
        .string();
    assert_eq!(named, "ok");
}

#[tokio::test]
async fn a_valid_arg_rejects_an_invalid_input() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ named(input: { name: \"\" }) }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors
        .iter()
        .next()
        .expect("an invalid input yields one error");
    assert_eq!(first.object().get("message").string(), "validation failed");
}
