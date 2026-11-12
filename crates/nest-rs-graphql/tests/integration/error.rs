//! Covers `src/error.rs`'s filing: the chain a field's answer keeps from its
//! client is filed once that answer is final, naming the root field it was met
//! under by `kind` and `field` — at the root field's site, at a field
//! resolver's, and, for an error no site answered (a resolver written by hand),
//! from the response's errors where the operation or a subscription's item
//! answers — and a filter answering it as the caller's files none.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::{
    Caught, ExceptionFilter, Layer, Problem, ToProblem, UnitContext, async_trait, code, injectable,
    module,
};
use nest_rs_graphql::async_graphql::futures_util::stream as futures_stream;
use nest_rs_graphql::async_graphql::{Object, Result, SimpleObject};
use nest_rs_graphql::{GraphqlModule, Opaque, operations, problem_error, resolver};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

/// What the ledger answers every field below, its words the operator's alone.
const LEDGER_REFUSED: &str = "the ledger at the eu-west warehouse refused";

fn settled() -> std::result::Result<i32, std::io::Error> {
    Err(std::io::Error::other(LEDGER_REFUSED))
}

/// The ledger's own failure, which says a `503`.
#[derive(Debug, thiserror::Error)]
#[error("{LEDGER_REFUSED}")]
struct LedgerDown;

impl ToProblem for LedgerDown {
    fn to_problem(&self) -> Option<Problem> {
        Some(Problem::new(503, code::UNAVAILABLE))
    }
}

/// Answers any failure as the caller's `409`.
#[injectable]
#[derive(Default, Layer)]
struct AsConflict;

#[async_trait]
impl ExceptionFilter for AsConflict {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(409, code::CONFLICT))
    }
}

/// Answers any failure as a `503`, the server unable to serve it in time.
#[injectable]
#[derive(Default, Layer)]
struct AsUnavailable;

#[async_trait]
impl ExceptionFilter for AsUnavailable {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(503, code::UNAVAILABLE))
    }
}

/// An object whose resolver no framework site runs.
struct Shelf;

/// Whether [`Shelf::pending`] began resolving, which it never ends.
static SHELF_PENDING: AtomicBool = AtomicBool::new(false);

#[Object]
impl Shelf {
    async fn book(&self) -> Result<i32> {
        settled().opaque()
    }

    async fn pending(&self) -> i32 {
        SHELF_PENDING.store(true, Ordering::SeqCst);
        std::future::pending().await
    }
}

/// An object a `#[field_resolver]` extends.
#[derive(SimpleObject)]
#[graphql(complex)]
struct Card {
    title: &'static str,
}

#[resolver]
struct LedgerResolver;

#[operations]
impl LedgerResolver {
    #[query]
    #[public]
    async fn failed(&self) -> std::result::Result<i32, std::io::Error> {
        settled()
    }

    #[query]
    #[public]
    async fn opaqued(&self) -> Result<i32> {
        settled().opaque()
    }

    #[query]
    #[public]
    async fn outage(&self) -> Result<i32> {
        Err(problem_error(&LedgerDown))
    }

    #[mutation]
    #[public]
    #[use_exception_filters(AsConflict)]
    async fn conflicted(&self) -> Result<i32> {
        settled().opaque()
    }

    #[mutation]
    #[public]
    #[use_exception_filters(AsUnavailable)]
    async fn unavailable(&self) -> Result<i32> {
        settled().opaque()
    }

    #[query]
    #[public]
    async fn shelf(&self) -> Shelf {
        Shelf
    }

    #[query]
    #[public]
    async fn card(&self) -> Card {
        Card { title: "ledger" }
    }

    #[field_resolver]
    async fn balance(&self, parent: &Card) -> Result<i32> {
        let Card { title } = parent;
        Ok(settled().opaque()? + i32::try_from(title.len()).unwrap_or_default())
    }

    #[subscription]
    #[public]
    async fn shelves(&self) -> impl futures_stream::Stream<Item = Shelf> {
        futures_stream::iter([Shelf])
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [AsConflict, AsUnavailable, LedgerResolver],
)]
struct LedgerModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<LedgerModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn post(app: &TestApp, query: &str) -> Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

/// The one line `logs` holds under `message`, at its level, naming the root
/// field `field` of an operation of `kind` and saying the ledger's refusal.
fn assert_names(logs: &LogCapture, message: &str, kind: &str, field: &str) {
    let line = logs.expect_one(nest_rs_graphql::TARGET, message);
    let level = match message.ends_with("unavailable") {
        true => "warn",
        false => "error",
    };
    assert_eq!(line.level, level, "{field}");
    assert_eq!(
        line.field("error").as_deref(),
        Some(LEDGER_REFUSED),
        "{field}"
    );
    assert_eq!(
        line.field("kind").as_deref(),
        Some(kind),
        "{field}: {:?}",
        line.fields
    );
    assert_eq!(
        line.field("field").as_deref(),
        Some(field),
        "{field}: {:?}",
        line.fields
    );
}

/// A root field's `Err` and `.opaque()`'s chain are filed where its site
/// answers, naming it by its operation's kind and the name its site declares,
/// whatever alias the document wrote.
#[tokio::test]
async fn a_withheld_chain_is_filed_naming_its_root_field() {
    let app = boot().await;
    for (document, field) in [
        ("{ failed }", "failed"),
        ("{ opaqued }", "opaqued"),
        ("{ aliased: opaqued }", "opaqued"),
    ] {
        let logs = LogCapture::install();
        let body = post(&app, document).await;
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "INTERNAL",
            "{body}"
        );
        assert!(!body.to_string().contains(LEDGER_REFUSED), "{body}");
        assert_names(&logs, "graphql operation failed", "query", field);
    }
}

/// `problem_error` in a body keeps the chain its `5xx` withholds for the site,
/// which files it at `warn` behind a `503`, naming the field.
#[tokio::test]
async fn a_problem_error_keeps_its_chain_for_the_site() {
    let app = boot().await;
    let logs = LogCapture::install();
    let body = post(&app, "{ outage }").await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "UNAVAILABLE",
        "{body}"
    );
    logs.expect_none(nest_rs_graphql::TARGET, "graphql operation failed");
    assert_names(&logs, "graphql operation unavailable", "query", "outage");
}

/// A filter answering `.opaque()`'s failure as the caller's `409` withholds
/// nothing, so nothing is filed.
#[tokio::test]
async fn a_filter_answering_4xx_over_opaque_files_nothing() {
    let app = boot().await;
    let logs = LogCapture::install();
    let body = post(&app, "mutation { conflicted }").await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "CONFLICT",
        "{body}"
    );
    logs.expect_none(nest_rs_graphql::TARGET, "graphql operation failed");
    logs.expect_none(nest_rs_graphql::TARGET, "graphql operation unavailable");
}

/// A filter answering `.opaque()`'s failure `503` withholds the chain it kept,
/// filed at the `503`'s level, `warn`.
#[tokio::test]
async fn a_filter_answering_503_over_opaque_files_its_chain_at_warn() {
    let app = boot().await;
    let logs = LogCapture::install();
    let body = post(&app, "mutation { unavailable }").await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "UNAVAILABLE",
        "{body}"
    );
    logs.expect_none(nest_rs_graphql::TARGET, "graphql operation failed");
    assert_names(
        &logs,
        "graphql operation unavailable",
        "mutation",
        "unavailable",
    );
}

/// A resolver no framework site runs keeps `.opaque()`'s chain on the error it
/// returns, filed from the operation's answer naming the root field it
/// resolved under; a `#[field_resolver]`'s site files its own the same way.
#[tokio::test]
async fn a_nested_resolvers_chain_is_filed_naming_its_root_field() {
    let app = boot().await;
    for (document, field) in [
        ("{ s: shelf { book } }", "shelf"),
        ("{ card { balance } }", "card"),
    ] {
        let logs = LogCapture::install();
        let body = post(&app, document).await;
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "INTERNAL",
            "{body}"
        );
        assert_names(&logs, "graphql operation failed", "query", field);
    }
}

/// A subscription's item a resolver no site runs failed in files its chain
/// where the item answers, naming the subscription.
#[tokio::test]
async fn a_subscription_items_chain_is_filed_naming_its_subscription() {
    let app = boot().await;
    let logs = LogCapture::install();
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("1", "subscription { shelves { book } }");
    let item = socket
        .next_item("1")
        .await
        .expect("the subscription streams");
    assert_eq!(
        item["errors"][0]["extensions"]["code"], "INTERNAL",
        "{item}"
    );
    assert_names(&logs, "graphql operation failed", "subscription", "shelves");
}

/// A subscription its client stops while an item resolves files what that
/// item's errors kept, the item never answered, naming the subscription. The
/// in-process socket runs while the test reads it.
#[tokio::test(start_paused = true)]
async fn a_subscription_stopped_mid_item_files_the_chain_its_errors_kept() {
    let app = boot().await;
    let logs = LogCapture::install();
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("1", "subscription { shelves { book pending } }");
    let item = socket.next_message_within(Duration::from_secs(1)).await;
    assert_eq!(item, None, "the item still resolves");
    assert!(SHELF_PENDING.load(Ordering::SeqCst), "the item began");
    logs.expect_none(nest_rs_graphql::TARGET, "graphql operation failed");

    socket.stop("1");
    let completed = socket.next_message_within(Duration::from_secs(1)).await;
    assert_eq!(completed, Some(json!({ "id": "1", "type": "complete" })));
    assert_names(&logs, "graphql operation failed", "subscription", "shelves");
}
