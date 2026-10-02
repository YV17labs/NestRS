//! The unit of work this edge opens per dispatched field, and the line it files.
//!
//! Until `graphql.operation` existed, every query and every mutation in a
//! deployment was one line — the `POST /graphql` the HTTP edge filed for the
//! whole document — so which field ran, which one failed and how long any of
//! them took were all unanswerable from the console. These assertions are that
//! statement, executed.

use nest_rs_core::module;
use nest_rs_graphql::async_graphql::{self, Context, Result as GqlResult};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_testing::{LogCapture, TestApp};

/// A parent object with a resolved field, so the `#[field_resolver]` role is in
/// the population rather than asserted about in the abstract.
#[derive(async_graphql::SimpleObject)]
#[graphql(complex)]
struct Note {
    body: String,
}

/// A payload whose name ends in `Result` — an ordinary object, which the
/// wrapper once took for a `Result` and refused to compile.
#[derive(async_graphql::SimpleObject)]
struct SearchResult {
    hits: i32,
}

/// A developer's own error: `Display`, so async-graphql can carry it, and no
/// `From<async_graphql::Error>` — which the wrapper's chain once demanded.
#[derive(Debug)]
struct LookupError;

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nothing by that name")
    }
}

#[resolver]
struct NoteResolver;

#[operations]
impl NoteResolver {
    #[query]
    #[public]
    async fn note(&self) -> async_graphql::Result<Note> {
        Ok(Note {
            body: "hello".into(),
        })
    }

    /// The failing half: an operation the framework cannot see the reason for
    /// still has to file `outcome = error`, or the line reports only the paths
    /// that were never in doubt.
    #[query]
    #[public]
    async fn refused(&self) -> async_graphql::Result<String> {
        Err(async_graphql::Error::new("no"))
    }

    /// A `Result` under another name — `use async_graphql::Result as
    /// GqlResult`, the idiom that keeps `std`'s in scope — fails as the spelled
    /// one does.
    #[query]
    #[public]
    async fn refused_by_alias(&self) -> GqlResult<String> {
        Err(async_graphql::Error::new("no"))
    }

    #[query]
    #[public]
    async fn search(&self) -> SearchResult {
        SearchResult { hits: 3 }
    }

    #[query]
    #[public]
    async fn look_up(&self) -> Result<i32, LookupError> {
        Err(LookupError)
    }

    #[mutation]
    #[public]
    async fn touch(&self) -> async_graphql::Result<bool> {
        Ok(true)
    }

    #[field_resolver]
    async fn shout(&self, parent: &Note, _ctx: &Context<'_>) -> async_graphql::Result<String> {
        Ok(parent.body.to_uppercase())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [NoteResolver],
)]
struct OperationTestModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<OperationTestModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn post(app: &TestApp, query: &str) {
    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .assert_status_is_ok();
}

fn lines(logs: &LogCapture) -> Vec<nest_rs_testing::CapturedEvent> {
    logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::OPERATION,
    )
}

#[tokio::test]
async fn every_dispatched_field_files_one_line_naming_itself() {
    let logs = LogCapture::install();
    let app = boot().await;

    post(&app, "{ note { body shout } }").await;

    let served = lines(&logs);
    // Two units: the root query, and the field resolver dispatched under it.
    // `body` is async-graphql's own accessor on `Note` and this crate never sees
    // it dispatched, which is the honest boundary rather than a gap.
    let named: Vec<(Option<String>, Option<String>)> = served
        .iter()
        .map(|line| (line.field("role"), line.field("operation")))
        .collect();
    assert!(
        named.contains(&(Some("query".into()), Some("note".into()))),
        "the root query names itself: {named:?}",
    );
    assert!(
        named.contains(&(Some("field".into()), Some("shout".into()))),
        "a field resolver is a dispatched unit of work too: {named:?}",
    );
    assert!(
        served
            .iter()
            .all(|line| line.field("duration_ms").is_some()),
        "every line is timed: {served:?}",
    );
    assert!(
        served
            .iter()
            .all(|line| line.field("outcome").as_deref() == Some(nest_rs_core::operation_log::OK)),
        "{served:?}",
    );
}

#[tokio::test]
async fn a_mutation_is_the_same_unit_under_its_own_role() {
    let logs = LogCapture::install();
    let app = boot().await;

    post(&app, "mutation { touch }").await;

    let served = lines(&logs);
    let touch = served
        .iter()
        .find(|line| line.field("operation").as_deref() == Some("touch"))
        .unwrap_or_else(|| panic!("the mutation files a line: {served:?}"));
    assert_eq!(touch.field("role").as_deref(), Some("mutation"));
}

#[tokio::test]
async fn a_failing_operation_says_so() {
    let logs = LogCapture::install();
    let app = boot().await;

    post(&app, "{ refused }").await;

    let served = lines(&logs);
    let refused = served
        .iter()
        .find(|line| line.field("operation").as_deref() == Some("refused"))
        .unwrap_or_else(|| panic!("the failing query files a line: {served:?}"));
    assert_eq!(
        refused.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "a GraphQL error is answered with a 200, so the HTTP line alone reports \
         a request that failed as one that succeeded: {served:?}",
    );
}

/// Fallibility is read from what a method returns, never from what its type
/// is called: a payload named `SearchResult` is a value and answers, a `Result`
/// renamed on import fails like a spelled one — on the line and to the client —
/// and a developer's own error type owes the wrapper nothing beyond what
/// async-graphql asks of it.
#[tokio::test]
async fn a_return_is_fallible_by_its_type_never_by_its_name() {
    let logs = LogCapture::install();
    let app = boot().await;

    let body: serde_json::Value = serde_json::from_str(
        &app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({
                "query": "{ search { hits } refusedByAlias lookUp }"
            }))
            .send()
            .await
            .0
            .into_body()
            .into_string()
            .await
            .expect("a GraphQL response body"),
    )
    .expect("a GraphQL response is JSON");
    assert_eq!(body["data"]["search"]["hits"], 3, "{body}");
    let messages: Vec<&str> = body["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(|e| e["message"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        messages.contains(&"no") && messages.contains(&"nothing by that name"),
        "both failures reach the client in their own words: {body}",
    );

    let served = lines(&logs);
    let outcome = |operation: &str| {
        served
            .iter()
            .find(|line| line.field("operation").as_deref() == Some(operation))
            .unwrap_or_else(|| panic!("`{operation}` files a line: {served:?}"))
            .field("outcome")
    };
    assert_eq!(
        outcome("search").as_deref(),
        Some(nest_rs_core::operation_log::OK)
    );
    for failed in ["refusedByAlias", "lookUp"] {
        assert_eq!(
            outcome(failed).as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
            "`{failed}` failed, and its line says so: {served:?}",
        );
    }
}

#[tokio::test]
async fn the_unit_is_a_child_of_the_request_that_carried_the_document() {
    let logs = LogCapture::install();
    let app = boot().await;

    post(&app, "{ note { shout } }").await;

    let spans: Vec<_> = logs
        .spans()
        .into_iter()
        .filter(|span| {
            span.target == nest_rs_graphql::TARGET && span.name == nest_rs_graphql::unit::OPERATION
        })
        .collect();
    assert!(!spans.is_empty(), "the unit opens a span of its own");
    assert!(
        spans
            .iter()
            .all(|span| span.field("parent_span_id").is_some()),
        "each names the HTTP request that carried it — the causal edge a flat id \
         could not express: {spans:?}",
    );
    // Two fields dispatched, two units, two span ids: a document is not one
    // unit of work, which is the whole reason the HTTP request's line could not
    // answer for it.
    let ids: std::collections::HashSet<_> = spans
        .iter()
        .filter_map(|span| span.field("span_id"))
        .collect();
    assert_eq!(
        ids.len(),
        spans.len(),
        "no two units share a span id: {spans:?}"
    );
    assert!(
        spans
            .iter()
            .all(|span| span.field("graphql.field.name").is_some()),
        "the span carries what the line carries, in the conventions' dotted \
         shape: {spans:?}",
    );
}
