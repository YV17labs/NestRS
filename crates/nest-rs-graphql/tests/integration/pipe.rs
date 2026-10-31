//! Per-argument pipes on GraphQL operations: a `Piped<P>` parameter exposes
//! the pipe's input on the wire, runs the pipe before the body, and surfaces a
//! rejection as a GraphQL error. An argument whose type declares rules is
//! validated there unasked, an optional one when it is given, and so is a
//! `#[field_resolver]`'s.

use std::sync::atomic::{AtomicU32, Ordering};

use async_graphql::{InputObject, SimpleObject};
use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_pipes::{ParseArray, Pipe, PipeError, Piped, Trim};
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
    /// `Piped<Trim>`: the SDL arg is `String`; the body sees it trimmed.
    #[query]
    #[public]
    async fn trimmed(&self, raw: Piped<Trim>) -> async_graphql::Result<String> {
        Ok(raw.into_inner())
    }

    /// A rejecting pipe surfaces as a GraphQL error, never reaching the body.
    #[query]
    #[public]
    async fn checked(&self, raw: Piped<Reject>) -> async_graphql::Result<String> {
        Ok(raw.into_inner())
    }

    /// The same rejection from a bare-return operation: the wrapper answers a
    /// `Result` whatever the method returns, so the pipe needs nothing of it.
    #[query]
    #[public]
    async fn checked_bare(&self, raw: Piped<Reject>) -> String {
        raw.into_inner()
    }

    /// A list whose items must parse: a refused item is said, never quoted.
    #[query]
    #[public]
    async fn ids(&self, raw: Piped<ParseArray<u64>>) -> async_graphql::Result<String> {
        Ok(format!("{:?}", raw.into_inner()))
    }

    /// A bare input object whose type declares rules.
    #[query]
    #[public]
    async fn named(&self, input: NameInput) -> async_graphql::Result<String> {
        Ok(input.name)
    }

    /// The same input, optional: validated when it is given.
    #[query]
    #[public]
    async fn maybe_named(&self, input: Option<NameInput>) -> async_graphql::Result<String> {
        Ok(input.map_or_else(|| "nobody".to_owned(), |input| input.name))
    }
}

#[derive(SimpleObject)]
#[graphql(complex)]
struct Shelf {
    id: i32,
}

/// Counts the field resolver bodies that ran, so a refusal is seen to run
/// before it.
static LABELLED: AtomicU32 = AtomicU32::new(0);

#[resolver]
struct ShelfResolver;

#[operations]
impl ShelfResolver {
    #[query]
    #[public]
    async fn shelf(&self) -> Shelf {
        Shelf { id: 1 }
    }

    /// A field resolver's argument whose type declares rules.
    #[field_resolver]
    async fn label(&self, parent: &Shelf, input: NameInput) -> String {
        LABELLED.fetch_add(1, Ordering::SeqCst);
        format!("{}:{}", parent.id, input.name)
    }
}

#[module(providers = [PipeResolver, ShelfResolver])]
struct PipeFeatureModule;

#[module(imports = [GraphqlModule, PipeFeatureModule])]
struct AppWithPipes;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AppWithPipes>()
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

/// The first error's message for `query`.
async fn first_error(query: &str) -> String {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors.iter().next().expect("the query yields an error");
    first.object().get("message").string().to_owned()
}

#[tokio::test]
async fn a_rejecting_pipe_surfaces_a_graphql_error() {
    assert_eq!(
        first_error("{ checked(raw: \"whatever\") }").await,
        "bad input"
    );
}

#[tokio::test]
async fn a_rejecting_pipe_surfaces_from_a_bare_return_operation() {
    assert_eq!(
        first_error("{ checkedBare(raw: \"whatever\") }").await,
        "bad input"
    );
}

#[tokio::test]
async fn an_optional_input_is_validated_when_it_is_given() {
    assert_eq!(
        first_error("{ maybeNamed(input: { name: \"\" }) }").await,
        "validation failed"
    );

    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ maybeNamed }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let named = json
        .value()
        .object()
        .get("data")
        .object()
        .get("maybeNamed")
        .string();
    assert_eq!(named, "nobody");
}

#[tokio::test]
async fn an_input_keeping_its_rules_is_accepted() {
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
async fn an_input_breaking_its_rules_is_refused_before_the_body() {
    assert_eq!(
        first_error("{ named(input: { name: \"\" }) }").await,
        "validation failed"
    );
}

#[tokio::test]
async fn a_field_resolver_argument_breaking_its_rules_is_a_field_error_before_the_body() {
    let app = boot().await;
    let ask = |name: &'static str| {
        let query = format!("{{ shelf {{ id label(input: {{ name: \"{name}\" }}) }} }}");
        let app = &app;
        async move {
            let resp = app
                .http()
                .post("/graphql")
                .body_json(&serde_json::json!({ "query": query }))
                .send()
                .await;
            resp.assert_status_is_ok();
            resp.json().await.value().deserialize::<serde_json::Value>()
        }
    };

    let refused = ask("").await;
    let error = &refused["errors"][0];
    assert_eq!(error["message"], "validation failed", "{refused}");
    assert_eq!(
        error["extensions"]["code"],
        nest_rs_core::problem::code::INVALID_ARGUMENT.as_str(),
        "{refused}",
    );
    assert_eq!(
        error["extensions"]["errors"]["name"][0]["code"], "length",
        "{refused}"
    );
    assert_eq!(error["path"], serde_json::json!(["shelf", "label"]));
    assert_eq!(LABELLED.load(Ordering::SeqCst), 0, "the body ran");

    let kept = ask("ok").await;
    assert_eq!(kept["data"]["shelf"]["label"], "1:ok", "{kept}");
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
