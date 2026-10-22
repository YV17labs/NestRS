//! Covers `src/operation_units.rs`: every operation a request carries runs as
//! a unit of its own through the binding the mount resolves — a query on the
//! pool, a mutation in one transaction whose root fields each run in an atomic
//! block settled on the field's own answer: a field that answers `null` for an
//! error rolls back alone, and one whose `null` reaches `data` rolls the whole
//! operation back.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nest_rs_core::module;
use nest_rs_database::{
    AtomicBlock, AtomicError, Executor, OpenUnit, Settlement, TransactionPolicy, Unhonoured,
    UnitOfWork, UnitSpec,
};
use nest_rs_graphql::async_graphql::{Object, Result};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};
use tokio::time::Instant;

/// What the binding was asked to do, in order.
#[derive(Default)]
struct Journal(Mutex<Vec<String>>);

impl Journal {
    fn note(&self, entry: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

    /// Every entry, the request's own unit included.
    fn all(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The entries of the units the request carries, without the request's
    /// own pool unit around them.
    fn read(&self) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|entry| !entry.contains(CARRIER))
            .collect()
    }
}

/// The unit of the HTTP request carrying the operations, which runs on the
/// pool around them.
const CARRIER: &str = "http.request";

/// How the recording binding settles a unit.
#[derive(Clone, Copy)]
enum Settles {
    At(Duration),
    Unhonoured,
}

/// A binding that journals every unit it opens and settles, and every atomic
/// block its executor opens.
struct Recording {
    journal: Arc<Journal>,
    settles: Settles,
    /// Whether its executors offer an atomic block.
    blocks: bool,
}

struct Opened {
    journal: Arc<Journal>,
    unit: &'static str,
    policy: TransactionPolicy,
    settles: Settles,
    blocks: bool,
}

/// The executor a unit installs, naming its policy.
struct Installed {
    journal: Arc<Journal>,
    policy: TransactionPolicy,
    blocks: bool,
}

/// An atomic block of an [`Installed`] executor.
struct Block {
    journal: Arc<Journal>,
}

fn policy_name(policy: TransactionPolicy) -> &'static str {
    match policy {
        TransactionPolicy::PerUnit => "per_unit",
        _ => "pool",
    }
}

impl UnitOfWork for Recording {
    fn open(&self, spec: &UnitSpec) -> std::result::Result<Box<dyn OpenUnit>, Unhonoured> {
        self.journal.note(format!(
            "open {} {}",
            spec.unit.name(),
            policy_name(spec.policy)
        ));
        Ok(Box::new(Opened {
            journal: Arc::clone(&self.journal),
            unit: spec.unit.name(),
            policy: spec.policy,
            settles: self.settles,
            blocks: self.blocks,
        }))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

impl OpenUnit for Opened {
    fn executor(&self) -> Arc<dyn Executor> {
        Arc::new(Installed {
            journal: Arc::clone(&self.journal),
            policy: self.policy,
            blocks: self.blocks,
        })
    }

    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        let Self {
            journal,
            unit,
            settles,
            ..
        } = *self;
        Box::pin(async move {
            // The request carrying the operations settles at once: what a test
            // sets is how an operation's unit settles.
            if unit == CARRIER {
                journal.note(format!("settle {unit} {ok}"));
                return Settlement::Settled;
            }
            match settles {
                Settles::At(after) => {
                    tokio::time::sleep(after).await;
                    journal.note(format!("settle {unit} {ok}"));
                    Settlement::Settled
                }
                Settles::Unhonoured => {
                    journal.note(format!("settle {unit} {ok}"));
                    Settlement::Unhonoured(Unhonoured::deterministic("refused by the test"))
                }
            }
        })
    }
}

impl Executor for Installed {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        self.blocks.then(|| {
            Arc::new(Block {
                journal: Arc::clone(&self.journal),
            }) as Arc<dyn AtomicBlock>
        })
    }
}

impl Executor for Block {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl AtomicBlock for Block {
    fn settle(
        self: Arc<Self>,
        ok: bool,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), AtomicError>> + Send>> {
        self.journal.note(format!(
            "block {}",
            if ok { "released" } else { "rolled back" }
        ));
        Box::pin(std::future::ready(Ok(())))
    }
}

/// The policy of the unit the field runs in, read off the ambient executor:
/// `block` inside a root field's atomic block.
fn ambient_policy() -> String {
    let Some(executor) = nest_rs_database::current_executor() else {
        return "none".into();
    };
    if executor.as_any().is::<Block>() {
        return "block".into();
    }
    match executor.as_any().downcast_ref::<Installed>() {
        Some(installed) => policy_name(installed.policy).into(),
        None => "unknown".into(),
    }
}

/// A shape whose fields fail below a root field: `broken` is nullable and holds
/// its own `null`, `strict` is Non-Null and hands it to its parent.
struct Shape;

#[Object]
impl Shape {
    async fn broken(&self) -> Result<Option<i32>> {
        Err("the nested field failed".into())
    }

    async fn strict(&self) -> Result<i32> {
        Err("the non-null nested field failed".into())
    }

    async fn fine(&self) -> i32 {
        1
    }

    async fn inner(&self) -> Option<Shape> {
        Some(Shape)
    }
}

#[resolver]
struct ProbeResolver;

#[operations]
impl ProbeResolver {
    #[query]
    #[public]
    async fn policy(&self) -> String {
        tokio::task::yield_now().await;
        ambient_policy()
    }

    #[mutation]
    #[public]
    async fn write(&self, fail: bool) -> Result<String> {
        tokio::task::yield_now().await;
        match fail {
            true => Err("the write was refused".into()),
            false => Ok(ambient_policy()),
        }
    }

    #[mutation]
    #[public]
    async fn maybe_write(&self, fail: bool) -> Result<Option<String>> {
        tokio::task::yield_now().await;
        match fail {
            true => Err("the write was refused".into()),
            false => Ok(Some(ambient_policy())),
        }
    }

    #[mutation]
    #[public]
    async fn shaped(&self) -> Shape {
        Shape
    }

    #[mutation]
    #[public]
    async fn maybe_shaped(&self) -> Option<Shape> {
        Some(Shape)
    }

    #[mutation]
    #[public]
    async fn slow_write(&self) -> String {
        tokio::time::sleep(Duration::from_secs(30)).await;
        ambient_policy()
    }

    #[query]
    #[public]
    async fn slow_read(&self) -> String {
        tokio::time::sleep(Duration::from_secs(30)).await;
        ambient_policy()
    }

    #[query]
    #[public]
    async fn explode(&self) -> bool {
        tokio::task::yield_now().await;
        panic!("the resolver exploded")
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [ProbeResolver])]
struct ProbeModule;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            request_timeout: Some(Duration::from_secs(1)),
            ..HttpConfig::default()
        }),
        GraphqlModule::for_root(None),
    ],
    providers = [ProbeResolver],
)]
struct DeadlineModule;

async fn boot_with<M: nest_rs_core::Module + 'static>(settles: Settles) -> (TestApp, Arc<Journal>) {
    boot_over::<M>(settles, true).await
}

async fn boot_over<M: nest_rs_core::Module + 'static>(
    settles: Settles,
    blocks: bool,
) -> (TestApp, Arc<Journal>) {
    let journal = Arc::new(Journal::default());
    let app = TestApp::builder()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Recording {
            journal: Arc::clone(&journal),
            settles,
            blocks,
        }))
        .module::<M>()
        .build()
        .await
        .expect("the schema boots over a seeded binding");
    (app, journal)
}

async fn boot() -> (TestApp, Arc<Journal>) {
    boot_with::<ProbeModule>(Settles::At(Duration::ZERO)).await
}

async fn post(app: &TestApp, body: Value) -> Value {
    let resp = app.http().post("/graphql").body_json(&body).send().await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

#[tokio::test]
async fn a_query_runs_as_a_unit_of_its_own_on_the_pool() {
    let (app, journal) = boot().await;
    let reply = post(&app, json!({ "query": "{ policy }" })).await;
    assert_eq!(reply["data"]["policy"], "pool", "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation pool",
            "settle graphql.operation true"
        ]
    );
}

#[tokio::test]
async fn the_request_carrying_operations_runs_on_the_pool_around_them() {
    let (app, journal) = boot().await;
    let reply = post(&app, json!({ "query": "mutation { write(fail: false) }" })).await;
    assert_eq!(reply["data"]["write"], "block", "{reply}");
    assert_eq!(
        journal.all(),
        [
            "open http.request pool",
            "open graphql.operation per_unit",
            "block released",
            "settle graphql.operation true",
            "settle http.request true",
        ],
        "the request holds no transaction; the operation it carries opens its own",
    );
}

#[tokio::test]
async fn a_mutation_runs_as_one_transaction_with_its_root_field_in_a_block() {
    let (app, journal) = boot().await;
    let reply = post(&app, json!({ "query": "mutation { write(fail: false) }" })).await;
    assert_eq!(reply["data"]["write"], "block", "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block released",
            "settle graphql.operation true",
        ]
    );
}

#[tokio::test]
async fn a_failing_root_field_answers_null_rolls_back_alone_and_the_operation_commits_the_others() {
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({
            "query": "mutation { a: maybeWrite(fail: false) b: maybeWrite(fail: true) c: maybeWrite(fail: false) }"
        }),
    )
    .await;
    assert_eq!(
        reply["data"],
        json!({ "a": "block", "b": null, "c": "block" }),
        "{reply}"
    );
    assert_eq!(reply["errors"][0]["path"], json!(["b"]), "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block released",
            "block rolled back",
            "block released",
            "settle graphql.operation true",
        ],
        "every field that executed reported its own outcome, so the operation commits"
    );
}

/// The root fields `#[operations]` dispatched, by name, in order: each files
/// one `graphql.field` line.
fn dispatched(logs: &LogCapture) -> Vec<String> {
    logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::FIELD.name(),
    )
    .into_iter()
    .filter_map(|line| line.field("operation"))
    .collect()
}

/// A Non-Null root field's `null` reaches `data`: the answer reports nothing,
/// so nothing the operation ran stands, and no root field after it resolves.
#[tokio::test]
async fn a_failing_non_null_root_field_nulls_the_data_rolls_the_operation_back_and_resolves_no_later_field()
 {
    let logs = LogCapture::install();
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({
            "query": "mutation { a: write(fail: false) b: write(fail: true) c: write(fail: false) }"
        }),
    )
    .await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    assert_eq!(reply["errors"].as_array().map(Vec::len), Some(1), "{reply}");
    assert_eq!(reply["errors"][0]["path"], json!(["b"]), "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block released",
            "block rolled back",
            "settle graphql.operation false",
        ],
        "`a` answered nothing the client reads, so it rolls back with the operation"
    );
    assert_eq!(dispatched(&logs), ["write", "write"], "`c` never resolves");
}

/// A Non-Null field's `null` below a Non-Null root field climbs through it to
/// `data`, as the root field's own error does.
#[tokio::test]
async fn a_non_null_field_failing_below_a_non_null_root_field_rolls_the_operation_back() {
    let logs = LogCapture::install();
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({
            "query": "mutation { a: write(fail: false) b: shaped { fine strict } c: write(fail: false) }"
        }),
    )
    .await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    assert_eq!(
        reply["errors"][0]["path"],
        json!(["b", "strict"]),
        "{reply}"
    );
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block released",
            "block rolled back",
            "settle graphql.operation false",
        ],
    );
    assert_eq!(dispatched(&logs), ["write", "shaped"], "`c` never resolves");
}

/// A Non-Null field's `null` below a nullable root field stops there: the root
/// field answers `null`, its block rolls back, and the others commit.
#[tokio::test]
async fn a_non_null_field_failing_below_a_nullable_root_field_nulls_it_and_rolls_back_its_block_alone()
 {
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({
            "query": "mutation { a: maybeWrite(fail: false) b: maybeShaped { fine strict } c: maybeWrite(fail: false) }"
        }),
    )
    .await;
    assert_eq!(
        reply["data"],
        json!({ "a": "block", "b": null, "c": "block" }),
        "{reply}"
    );
    assert_eq!(
        reply["errors"][0]["path"],
        json!(["b", "strict"]),
        "{reply}"
    );
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block released",
            "block rolled back",
            "block released",
            "settle graphql.operation true",
        ],
    );
}

/// A `null` that stays below the root field — a nullable field's own, or a
/// Non-Null field's held by a nullable parent — leaves the root field's answer
/// standing, so its work stands too.
#[tokio::test]
async fn an_error_whose_null_stays_below_the_root_field_releases_its_block() {
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({
            "query": "mutation { a: shaped { fine broken inner { strict } } b: maybeShaped { broken } }"
        }),
    )
    .await;
    assert_eq!(
        reply["data"],
        json!({
            "a": { "fine": 1, "broken": null, "inner": null },
            "b": { "broken": null },
        }),
        "{reply}"
    );
    assert_eq!(reply["errors"].as_array().map(Vec::len), Some(3), "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block released",
            "block released",
            "settle graphql.operation true",
        ],
        "each root field answered its value: none of its work was rolled back"
    );
}

#[tokio::test]
async fn a_batch_settles_each_mutation_on_its_own_answer() {
    let (app, journal) = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!([
            { "query": "mutation { write(fail: true) }" },
            { "query": "mutation { write(fail: false) }" },
        ]))
        .send()
        .await;
    resp.assert_status_is_ok();
    let replies: Value = resp.json().await.value().deserialize();
    assert_eq!(replies[0]["data"], Value::Null, "{replies}");
    assert_eq!(replies[1]["data"]["write"], "block", "{replies}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "block rolled back",
            "settle graphql.operation false",
            "open graphql.operation per_unit",
            "block released",
            "settle graphql.operation true",
        ],
    );
}

/// A driver offering no atomic block fails a mutation field closed; a Non-Null
/// one's `null` reaches `data`, so the fields after it do not run to fail too.
#[tokio::test]
async fn a_driver_offering_no_atomic_block_fails_the_first_root_field_and_resolves_no_later_one() {
    let (app, journal) = boot_over::<ProbeModule>(Settles::At(Duration::ZERO), false).await;
    let reply = post(
        &app,
        json!({ "query": "mutation { a: write(fail: false) b: write(fail: false) }" }),
    )
    .await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    let errors = reply["errors"].as_array().expect("one error");
    assert_eq!(errors.len(), 1, "{reply}");
    assert_eq!(errors[0]["path"], json!(["a"]), "{reply}");
    assert_eq!(errors[0]["extensions"]["code"], "INTERNAL", "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "settle graphql.operation false",
        ],
    );
}

#[tokio::test]
async fn a_batch_holding_a_mutation_runs_its_operations_in_order() {
    let (app, journal) = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!([
            { "query": "{ policy }" },
            { "query": "mutation { write(fail: false) }" },
        ]))
        .send()
        .await;
    resp.assert_status_is_ok();
    let replies: Value = resp.json().await.value().deserialize();
    assert_eq!(replies[0]["data"]["policy"], "pool", "{replies}");
    assert_eq!(replies[1]["data"]["write"], "block", "{replies}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation pool",
            "settle graphql.operation true",
            "open graphql.operation per_unit",
            "block released",
            "settle graphql.operation true",
        ],
        "one unit at a time: a batch never holds two pooled connections"
    );
}

#[tokio::test]
async fn a_batch_of_queries_runs_its_operations_at_once() {
    let (app, journal) = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!([{ "query": "{ policy }" }, { "query": "{ policy }" }]))
        .send()
        .await;
    resp.assert_status_is_ok();
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation pool",
            "open graphql.operation pool",
            "settle graphql.operation true",
            "settle graphql.operation true",
        ]
    );
}

#[tokio::test]
async fn an_operation_that_does_not_parse_runs_on_the_pool() {
    let (app, journal) = boot().await;
    let reply = post(&app, json!({ "query": "mutation {{{" })).await;
    assert!(reply["errors"].is_array(), "{reply}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation pool",
            "settle graphql.operation false"
        ]
    );
}

#[tokio::test]
async fn the_operation_selected_by_name_decides_the_policy() {
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({
            "query": "query Read { policy } mutation Write { write(fail: false) }",
            "operationName": "Read",
        }),
    )
    .await;
    assert_eq!(reply["data"]["policy"], "pool", "{reply}");
    assert_eq!(journal.read()[0], "open graphql.operation pool");
}

#[tokio::test]
async fn an_operation_whose_commit_is_not_honoured_answers_the_opaque_error_alone() {
    let (app, _) = boot_with::<ProbeModule>(Settles::Unhonoured).await;
    let reply = post(&app, json!({ "query": "mutation { write(fail: false) }" })).await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    let errors = reply["errors"].as_array().expect("one error");
    assert_eq!(errors.len(), 1, "{reply}");
    assert_eq!(errors[0]["message"], nest_rs_core::OPAQUE_CLIENT_MESSAGE);
    assert_eq!(errors[0]["extensions"]["code"], "INTERNAL", "{reply}");
}

/// Every `graphql.operation` line filed so far.
fn operation_lines(logs: &LogCapture) -> Vec<nest_rs_testing::CapturedEvent> {
    logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::OPERATION.name(),
    )
}

/// The span of the operation `name` names, which says what its line says.
fn operation_span(logs: &LogCapture, name: &str) -> nest_rs_testing::CapturedSpan {
    logs.spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_graphql::unit::OPERATION.name()
                && span.field("graphql.operation.name").as_deref() == Some(name)
        })
        .unwrap_or_else(|| panic!("the span of `{name}`: {:#?}", logs.spans()))
}

/// The span of `name`, a query, exports the outcome its line filed.
fn assert_query_span_failed(logs: &LogCapture, name: &str, outcome: &str) {
    let span = operation_span(logs, name);
    assert_eq!(
        span.field("graphql.operation.type").as_deref(),
        Some("query")
    );
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(outcome),
        "{:?}",
        span.fields
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
}

/// The outcome the line of the operation `name` filed, found by its span's
/// ids: the line names no operation.
fn outcome_of(
    logs: &LogCapture,
    lines: &[nest_rs_testing::CapturedEvent],
    name: &str,
) -> Option<String> {
    let span_id = operation_span(logs, name).field("span_id");
    lines
        .iter()
        .find(|line| line.span_id.is_some() && line.span_id == span_id)
        .unwrap_or_else(|| panic!("`{name}` files a line: {lines:#?}"))
        .field("outcome")
}

/// The line names the operation's type; its name is the document's, the
/// client's to write, so the line leaves it to the span, which the GraphQL
/// conventions ask to carry it.
#[tokio::test]
async fn each_operation_files_one_line_naming_its_type_and_its_span_its_name() {
    let logs = LogCapture::install();
    let (app, _) = boot().await;
    post(
        &app,
        json!({ "query": "mutation MarkedName { b: write(fail: true) }" }),
    )
    .await;
    post(&app, json!({ "query": "{ policy }" })).await;

    let lines = operation_lines(&logs);
    assert_eq!(lines.len(), 2, "{lines:#?}");
    for event in logs.events() {
        for (field, value) in &event.fields {
            assert!(
                !value.contains("Marked"),
                "{}: {field} = {value}",
                event.target
            );
        }
    }
    assert_eq!(
        lines[0].field("operation_type").as_deref(),
        Some("mutation")
    );
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "its response carries an error"
    );
    assert_eq!(lines[1].field("operation_type").as_deref(), Some("query"));
    assert_eq!(
        lines[1].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK)
    );

    let named = operation_span(&logs, "MarkedName");
    assert_eq!(
        named.field("graphql.operation.type").as_deref(),
        Some("mutation")
    );
    assert_eq!(
        named.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "{:?}",
        named.fields
    );
    assert_eq!(named.field("otel.status_code").as_deref(), Some("error"));
    let anonymous = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_graphql::unit::OPERATION.name()
                && span.field("graphql.operation.name").is_none()
        })
        .expect("the anonymous query's span");
    assert_eq!(
        anonymous.field("graphql.operation.type").as_deref(),
        Some("query")
    );
    assert_eq!(
        anonymous.field("error.type"),
        None,
        "an operation that answered whole exports no error"
    );
}

#[tokio::test(start_paused = true)]
async fn an_operation_whose_request_is_dropped_files_its_line_cancelled() {
    let logs = LogCapture::install();
    let (app, _) = boot().await;
    let sent = tokio::time::timeout(
        Duration::from_millis(300),
        app.http()
            .post("/graphql")
            .body_json(&json!({ "query": "query Waiting { slowRead }" }))
            .send(),
    )
    .await;
    assert!(sent.is_err(), "the request was dropped before it answered");

    let lines = operation_lines(&logs);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED)
    );
    assert_query_span_failed(&logs, "Waiting", nest_rs_core::operation_log::CANCELLED);
}

/// async-graphql does not catch a resolver that unwinds: the operation files
/// `panic` and resumes it, and the HTTP edge answers the request as it answers
/// a handler that unwound.
#[tokio::test]
async fn an_operation_whose_root_field_panics_files_its_line_panic() {
    let logs = LogCapture::install();
    let (app, _) = boot().await;
    app.http()
        .post("/graphql")
        .body_json(&json!({ "query": "query Exploding { explode }" }))
        .send()
        .await
        .assert_status(nest_rs_http::StatusCode::INTERNAL_SERVER_ERROR);

    let lines = operation_lines(&logs);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC)
    );
    assert_query_span_failed(&logs, "Exploding", nest_rs_core::operation_log::PANIC);
}

/// A batch of queries runs its operations at once, so one that unwinds tears
/// its sibling down with it; only it files `panic`.
#[tokio::test]
async fn an_operation_torn_down_by_a_sibling_that_panics_files_cancelled() {
    let logs = LogCapture::install();
    let (app, _) = boot().await;
    app.http()
        .post("/graphql")
        .body_json(&json!([
            { "query": "query Exploding { explode }" },
            { "query": "query Waiting { slowRead }" },
        ]))
        .send()
        .await
        .assert_status(nest_rs_http::StatusCode::INTERNAL_SERVER_ERROR);

    let lines = operation_lines(&logs);
    assert_eq!(lines.len(), 2, "one line per operation: {lines:#?}");
    assert_eq!(
        outcome_of(&logs, &lines, "Exploding").as_deref(),
        Some(nest_rs_core::operation_log::PANIC)
    );
    assert_eq!(
        outcome_of(&logs, &lines, "Waiting").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the sibling did not unwind"
    );
    assert_query_span_failed(&logs, "Exploding", nest_rs_core::operation_log::PANIC);
    assert_query_span_failed(&logs, "Waiting", nest_rs_core::operation_log::CANCELLED);
}

#[tokio::test(start_paused = true)]
async fn a_mutation_past_the_request_deadline_is_cut_inside_its_unit_and_answered_in_band() {
    let logs = LogCapture::install();
    let (app, journal) = boot_with::<DeadlineModule>(Settles::At(Duration::ZERO)).await;
    let started = Instant::now();
    let reply = post(&app, json!({ "query": "mutation Slow { slowWrite }" })).await;
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(1),
        "cut at the deadline"
    );
    assert_eq!(reply["data"], Value::Null, "{reply}");
    assert_eq!(
        reply["errors"][0]["extensions"]["code"], "UNAVAILABLE",
        "{reply}"
    );
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "settle graphql.operation false",
        ],
        "nothing it ran commits: the block was dropped unsettled, which its driver rolls back"
    );

    let cut = logs.expect_one(
        nest_rs_graphql::TARGET,
        "graphql operation not answered by the request deadline; it is cut",
    );
    assert_eq!(cut.level, "warn");
    assert_eq!(cut.field("operation_type").as_deref(), Some("mutation"));
    assert_eq!(
        cut.field("operation_name"),
        None,
        "the document's name is the client's"
    );
    assert_eq!(
        cut.field("timeout").as_deref(),
        Some("1s"),
        "the budget it ran out of"
    );
    let lines = operation_lines(&logs);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(
        cut.span_id, lines[0].span_id,
        "said under the operation's own ids, never the request's"
    );
}

#[tokio::test(start_paused = true)]
async fn a_mutations_commit_is_waited_for_past_the_request_deadline() {
    let (app, journal) =
        boot_with::<DeadlineModule>(Settles::At(Duration::from_millis(1500))).await;
    let started = Instant::now();
    let reply = post(&app, json!({ "query": "mutation { write(fail: false) }" })).await;
    assert_eq!(
        reply["data"]["write"], "block",
        "never cut while it commits: {reply}"
    );
    assert_eq!(started.elapsed(), Duration::from_millis(1500));
    assert_eq!(
        journal.read().last().map(String::as_str),
        Some("settle graphql.operation true")
    );
}

#[tokio::test(start_paused = true)]
async fn a_query_past_the_request_deadline_is_cut_by_the_edge() {
    let (app, _) = boot_with::<DeadlineModule>(Settles::At(Duration::ZERO)).await;
    let started = Instant::now();
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": "{ slowRead }" }))
        .send()
        .await;
    resp.assert_status(nest_rs_http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(started.elapsed(), Duration::from_secs(1));
}

/// No binding: a mutation resolves with no unit and no block, as before any
/// data layer.
#[tokio::test]
async fn with_no_binding_a_mutation_resolves_bare() {
    let app = TestApp::builder()
        .module::<ProbeModule>()
        .build()
        .await
        .expect("the schema boots with no data layer");
    let reply = post(&app, json!({ "query": "mutation { write(fail: false) }" })).await;
    assert_eq!(reply["data"]["write"], "none", "{reply}");
}

/// No binding, nothing to roll back: a Non-Null root field that nulls `data`
/// still leaves the fields after it unresolved, since the answer reports none.
#[tokio::test]
async fn with_no_binding_no_root_field_resolves_after_one_nulls_the_data() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<ProbeModule>()
        .build()
        .await
        .expect("the schema boots with no data layer");
    let reply = post(
        &app,
        json!({ "query": "mutation { a: write(fail: true) b: write(fail: false) }" }),
    )
    .await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    assert_eq!(dispatched(&logs), ["write"], "`b` never resolves");
}

/// The next text frame of a graphql-ws socket, parsed.
async fn next_message(socket: &mut nest_rs_testing::ws::WsSocket) -> Value {
    match socket.next_frame().await {
        Some(nest_rs_testing::ws::WsFrame::Text(text)) => {
            serde_json::from_str(&text).expect("a protocol message")
        }
        other => panic!("expected a protocol message, got {other:?}"),
    }
}

#[tokio::test]
async fn over_graphql_ws_each_mutation_runs_as_its_unit_inside_the_sockets() {
    let journal = Arc::new(Journal::default());
    let app = TestApp::builder()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Recording {
            journal: Arc::clone(&journal),
            settles: Settles::At(Duration::ZERO),
            blocks: true,
        }))
        .module::<ProbeModule>()
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut socket = app
        .socket("/graphql")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .connect()
        .await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");

    for (id, fail) in [("1", false), ("2", true)] {
        let query = format!("mutation {{ write(fail: {fail}) }}");
        socket
            .send_text(
                json!({ "id": id, "type": "subscribe", "payload": { "query": query } }).to_string(),
            )
            .await;
        let answer = next_message(&mut socket).await;
        assert_eq!(answer["type"], "next", "{answer}");
        assert_eq!(next_message(&mut socket).await["type"], "complete");
        match fail {
            false => assert_eq!(answer["payload"]["data"]["write"], "block", "{answer}"),
            true => assert!(answer["payload"]["errors"].is_array(), "{answer}"),
        }
    }

    assert_eq!(
        journal.read(),
        [
            "open graphql.subscription pool",
            "open graphql.operation per_unit",
            "block released",
            "settle graphql.operation true",
            "open graphql.operation per_unit",
            "block rolled back",
            "settle graphql.operation false",
        ],
        "the socket's pool unit holds no transaction; each mutation opens and settles its own, \
         on its own answer"
    );
}
