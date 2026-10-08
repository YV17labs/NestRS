//! Per-argument pipes on GraphQL operations: a `Piped<P, T>` / `Valid<T>`
//! parameter exposes the wire type `T`, runs the pipe before the body, and
//! surfaces a rejection as a GraphQL error.

use async_graphql::InputObject;
use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_pipes::{ParseArray, Pipe, PipeError, Piped, Trim, Valid};
use nest_rs_testing::TestApp;
use validator::Validate;

/// A pipe that always rejects.
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

#[operations]
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

    /// The same rejection from a bare-return operation: the wrapper answers a
    /// `Result` whatever the method returns, so the pipe needs nothing of it.
    #[query]
    #[public]
    async fn checked_bare(&self, raw: Piped<Reject, String>) -> String {
        raw.into_inner()
    }

    /// A list whose items must parse: a refused item is said, never quoted.
    #[query]
    #[public]
    async fn ids(&self, raw: Piped<ParseArray<u64>, String>) -> async_graphql::Result<String> {
        Ok(format!("{:?}", raw.into_inner()))
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
async fn a_rejecting_pipe_surfaces_from_a_bare_return_operation() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ checkedBare(raw: \"whatever\") }" }))
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

#[tokio::test]
async fn a_refused_list_item_is_never_quoted_in_the_error() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ids(raw: \"1,sk_live_51HsecretTOKEN,3\") }" }))
        .send()
        .await;

    resp.assert_status_is_ok();
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a readable body");
    assert!(
        !body.contains("sk_live"),
        "the reply quotes the item: {body}"
    );
    let reply: serde_json::Value = serde_json::from_str(&body).expect("a GraphQL response");
    assert!(
        reply["errors"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("u64")),
        "the refusal names what an item must be: {reply}",
    );
    let quoting: Vec<String> = logs
        .events()
        .into_iter()
        .filter(|event| event.fields.values().any(|value| value.contains("sk_live")))
        .map(|event| format!("{} {:?}", event.message, event.fields))
        .collect();
    assert!(quoting.is_empty(), "lines quoting the item: {quoting:#?}");
}
