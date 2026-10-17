//! The unit of work this edge opens per dispatched root field, and the line it
//! files.

use nest_rs_core::module;
use nest_rs_graphql::async_graphql::{self, Context, Result as GqlResult};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_testing::{LogCapture, TestApp};

/// A parent object, so the `#[field_resolver]` role is in the population.
#[derive(async_graphql::SimpleObject)]
#[graphql(complex)]
struct Note {
    body: String,
}

/// A payload whose name ends in `Result` — an ordinary object.
#[derive(async_graphql::SimpleObject)]
struct SearchResult {
    hits: i32,
}

/// A developer's own error, saying what its client may read through
/// `ToProblem` rather than through a `From<…> for async_graphql::Error`.
#[derive(Debug)]
struct LookupError;

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no note under that name")
    }
}

impl std::error::Error for LookupError {}

impl nest_rs_core::ToProblem for LookupError {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        Some(
            nest_rs_core::Problem::new(404, nest_rs_core::problem::code::NOT_FOUND)
                .with_detail("nothing by that name"),
        )
    }
}

/// Told when the waiting query has started, so its request is dropped while the
/// query runs.
static WAITING: tokio::sync::Notify = tokio::sync::Notify::const_new();

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

    #[query]
    #[public]
    async fn notes(&self) -> async_graphql::Result<Vec<Note>> {
        Ok(["a", "b", "c"]
            .into_iter()
            .map(|body| Note { body: body.into() })
            .collect())
    }

    #[query]
    #[public]
    async fn refused(&self) -> async_graphql::Result<String> {
        Err(async_graphql::Error::new("no"))
    }

    /// A `Result` under another name (`async_graphql::Result as GqlResult`).
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

    /// Waits on something that never comes: only dropping its request ends it.
    #[query]
    #[public]
    async fn waits(&self) -> String {
        WAITING.notify_one();
        std::future::pending::<()>().await;
        String::new()
    }

    #[mutation]
    #[public]
    async fn touch(&self) -> async_graphql::Result<bool> {
        Ok(true)
    }

    #[query]
    #[public]
    async fn slow(&self) -> async_graphql::Result<bool> {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Ok(true)
    }

    #[query]
    #[public]
    async fn explode(&self) -> async_graphql::Result<bool> {
        tokio::task::yield_now().await;
        panic!("the resolver exploded")
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
        nest_rs_graphql::unit::FIELD.name(),
    )
}

#[tokio::test]
async fn every_dispatched_root_field_files_one_line_naming_itself() {
    let logs = LogCapture::install();
    let app = boot().await;

    post(&app, "{ note { body shout } }").await;

    let served = lines(&logs);
    // `body` is async-graphql's own accessor, never dispatched by this crate.
    let named: Vec<(Option<String>, Option<String>)> = served
        .iter()
        .map(|line| (line.field("role"), line.field("operation")))
        .collect();
    assert_eq!(
        named,
        [(Some("query".into()), Some("note".into()))],
        "the root query names itself, and its field resolver files nothing",
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

/// A client controls how many parents a field resolver runs over, so a unit per
/// call would let one document file a line per row.
#[tokio::test]
async fn a_field_resolver_over_many_parents_files_no_line_of_its_own() {
    let logs = LogCapture::install();
    let app = boot().await;

    post(&app, "{ notes { body shout } }").await;

    let served = lines(&logs);
    assert_eq!(served.len(), 1, "one line, the root field's: {served:#?}");
    assert_eq!(served[0].field("operation").as_deref(), Some("notes"));
    assert_eq!(
        logs.find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_graphql::unit::OPERATION.name(),
        )
        .len(),
        1,
        "one line for the document's operation, which names no root field",
    );
    let field_spans = logs
        .spans()
        .into_iter()
        .filter(|span| span.name == nest_rs_graphql::unit::FIELD.name())
        .count();
    assert_eq!(field_spans, 1, "one span, the root field's");
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
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_graphql::unit::FIELD.name()
                && span.field("graphql.field.name").as_deref() == Some("refused")
        })
        .expect("the failing operation's span");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
}

#[tokio::test]
async fn a_dropped_graphql_request_exports_its_http_span_under_its_route() {
    let logs = LogCapture::install();
    let app = boot().await;

    tokio::select! {
        _ = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ waits }" }))
            .send() => panic!("the waiting query never answers"),
        () = WAITING.notified() => {}
    }

    let span = logs.expect_span("nest_rs::http", nest_rs_http::unit::REQUEST.name());
    assert_eq!(
        span.field("http.route").as_deref(),
        Some("/graphql"),
        "{:?}",
        span.fields
    );
    assert_eq!(span.field("otel.name").as_deref(), Some("POST /graphql"));
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
}

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
            span.target == nest_rs_graphql::TARGET
                && span.name == nest_rs_graphql::unit::FIELD.name()
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

#[tokio::test]
async fn a_field_whose_request_is_dropped_files_its_line_cancelled() {
    let logs = LogCapture::install();
    let app = boot().await;

    let sent = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ slow }" }))
            .send(),
    )
    .await;
    assert!(sent.is_err(), "the request was dropped before it answered");

    let filed = lines(&logs);
    assert_eq!(filed.len(), 1, "one line for the field: {filed:#?}");
    assert_eq!(filed[0].field("operation").as_deref(), Some("slow"));
    assert_eq!(
        filed[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    assert_field_span_failed(&logs, "slow", nest_rs_core::operation_log::CANCELLED);
}

/// async-graphql does not catch a resolver that unwinds: the field files
/// `panic` and resumes it, and the HTTP edge answers the request as it answers
/// a handler that unwound.
#[tokio::test]
async fn a_field_that_panics_files_its_line_panic() {
    let logs = LogCapture::install();
    let app = boot().await;

    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ explode }" }))
        .send()
        .await
        .assert_status(poem::http::StatusCode::INTERNAL_SERVER_ERROR);

    let filed = lines(&logs);
    assert_eq!(filed.len(), 1, "one line for the field: {filed:#?}");
    assert_eq!(filed[0].field("operation").as_deref(), Some("explode"));
    assert_eq!(
        filed[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
    );
    assert_field_span_failed(&logs, "explode", nest_rs_core::operation_log::PANIC);
}

fn assert_field_span_failed(logs: &LogCapture, field: &str, outcome: &str) {
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_graphql::unit::FIELD.name()
                && span.field("graphql.field.name").as_deref() == Some(field)
        })
        .unwrap_or_else(|| panic!("the span of `{field}`"));
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(outcome),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
}

/// A field that unwinds tears its siblings down with it; only it files `panic`.
#[tokio::test]
async fn a_field_torn_down_by_a_sibling_that_panics_files_cancelled() {
    let logs = LogCapture::install();
    let app = boot().await;

    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ slow explode }" }))
        .send()
        .await
        .assert_status(poem::http::StatusCode::INTERNAL_SERVER_ERROR);

    let filed = lines(&logs);
    let outcome = |operation: &str| {
        filed
            .iter()
            .find(|line| line.field("operation").as_deref() == Some(operation))
            .and_then(|line| line.field("outcome"))
    };
    assert_eq!(filed.len(), 2, "one line per field: {filed:#?}");
    assert_eq!(
        outcome("explode").as_deref(),
        Some(nest_rs_core::operation_log::PANIC)
    );
    assert_eq!(
        outcome("slow").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the sibling did not unwind: {filed:#?}",
    );
}

#[resolver]
#[use_guards(crate::FieldStoreDown)]
struct StoreDownResolver;

#[operations]
impl StoreDownResolver {
    #[query]
    #[public]
    async fn guarded(&self) -> async_graphql::Result<String> {
        Ok("never reached".into())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [StoreDownResolver, crate::FieldStoreDown],
)]
struct StoreDownModule;

/// A field guard that cannot evaluate the field files what failed on the
/// field's denial line, and the client reads the condition alone.
#[tokio::test]
async fn a_field_denial_keeping_its_cause_files_the_chain() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<StoreDownModule>()
        .await
        .expect("the schema boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ guarded }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await.value().deserialize::<serde_json::Value>();
    assert_eq!(
        body["errors"][0]["message"],
        nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
        "{body}",
    );
    assert!(!body.to_string().contains("refused"), "{body}");
    let event = logs.expect_one("nest_rs::layers", "guard denied the operation");
    assert_eq!(event.field("status").as_deref(), Some("503"));
    assert_eq!(
        event.field("error").as_deref(),
        Some(crate::STORE_DOWN_CHAIN)
    );
}
