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

use nest_rs_core::{Problem, code, module};
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

    async fn failed_below(&self) -> Result<Option<i32>> {
        Err(nest_rs_graphql::problem_error(&Problem::new(
            500,
            code::INTERNAL,
        )))
    }
}

/// An app's own class for a server failure no constant of the framework names.
const INSUFFICIENT_STORAGE: nest_rs_core::Code = nest_rs_core::Code::new("INSUFFICIENT_STORAGE");

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

    #[query]
    #[public]
    async fn hinted(&self, hint: Option<String>) -> bool {
        hint.is_some()
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

    #[query]
    #[public]
    async fn shape(&self) -> Option<Shape> {
        Some(Shape)
    }

    #[query]
    #[public]
    async fn conflict(&self) -> std::result::Result<Option<String>, Problem> {
        Err(Problem::new(409, code::CONFLICT))
    }

    #[query]
    #[public]
    async fn failed(&self) -> std::result::Result<Option<String>, Problem> {
        Err(Problem::new(500, code::INTERNAL))
    }

    #[query]
    #[public]
    async fn storage(&self) -> std::result::Result<Option<String>, Problem> {
        Err(Problem::new(507, INSUFFICIENT_STORAGE))
    }
}

#[module(imports = [GraphqlModule], providers = [ProbeResolver])]
struct ProbeModule;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            request_timeout: Some(Duration::from_secs(1)),
            ..HttpConfig::default()
        }),
        GraphqlModule,
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

/// async-graphql runs a root field once per selection, where the
/// specification runs one response key once: a mutation naming one root field
/// twice — under one key, through fragments, a copy a directive skips included
/// — would write twice and answer one of the two, so it is refused before any
/// root field runs.
#[tokio::test]
async fn a_mutation_selecting_one_root_field_twice_is_refused_before_any_runs() {
    let logs = LogCapture::install();
    let (app, journal) = boot().await;
    let documents = [
        (
            "mutation { a: maybeShaped { broken } a: maybeShaped { fine } }",
            r#""a""#,
        ),
        (
            "mutation { maybeWrite(fail: false) ... on Mutation { maybeWrite(fail: false) } }",
            r#""maybeWrite""#,
        ),
        (
            "mutation { ...A ...B } \
             fragment A on Mutation { w: maybeWrite(fail: false) } \
             fragment B on Mutation { w: maybeWrite(fail: false) }",
            r#""w""#,
        ),
        (
            "mutation { ...A ...A } fragment A on Mutation { maybeWrite(fail: false) }",
            r#""A""#,
        ),
    ];
    for (document, named) in documents {
        let reply = post(&app, json!({ "query": document })).await;
        assert_eq!(reply["data"], Value::Null, "{document}: {reply}");
        assert_eq!(
            reply["errors"].as_array().map(Vec::len),
            Some(1),
            "{document}: {reply}"
        );
        let refused = &reply["errors"][0];
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|message| message.contains(named)),
            "{document}: {reply}"
        );
        assert_eq!(
            refused["extensions"]["code"], "INVALID_ARGUMENT",
            "{document}: {reply}"
        );
    }
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "settle graphql.operation false"
        ]
        .repeat(documents.len()),
        "each refused operation is a unit that runs nothing and keeps nothing"
    );
    assert!(dispatched(&logs).is_empty(), "no root field resolves");

    let reply = post(&app, json!({ "query": "{ a: policy a: policy }" })).await;
    assert_eq!(
        reply["data"],
        json!({ "a": "pool" }),
        "a query writes nothing, so it runs as async-graphql runs it: {reply}"
    );
}

/// A document validation refuses says what validation says of it: the repeat
/// is refused only of a document that could have run.
#[tokio::test]
async fn a_mutation_selecting_one_root_field_twice_answers_validation_first() {
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({ "query": "mutation { a: write(fail: false) a: write(fail: false) unknownField }" }),
    )
    .await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    assert_eq!(reply["errors"].as_array().map(Vec::len), Some(1), "{reply}");
    assert_eq!(
        reply["errors"][0]["message"], r#"Unknown field "unknownField" on type "Mutation"."#,
        "{reply}"
    );
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "settle graphql.operation false"
        ],
    );
}

/// A copy a directive leaves out never runs, so it repeats nothing: the
/// document runs what `@skip` and `@include` keep, once.
#[tokio::test]
async fn a_copy_a_directive_leaves_out_repeats_no_root_field() {
    for (body, data) in [
        (
            json!({
                "query": "mutation($x: Boolean!) { \
                          maybeWrite(fail: false) @skip(if: $x) \
                          maybeWrite(fail: false) @include(if: $x) }",
                "variables": { "x": true },
            }),
            json!({ "maybeWrite": "block" }),
        ),
        (
            json!({
                "query": "mutation($x: Boolean = true) { \
                          maybeWrite(fail: false) @skip(if: $x) \
                          maybeWrite(fail: false) }",
            }),
            json!({ "maybeWrite": "block" }),
        ),
        (
            json!({
                "query": "mutation { \
                          a: maybeShaped { fine } a: maybeShaped @skip(if: true) { strict } }",
            }),
            json!({ "a": { "fine": 1 } }),
        ),
        (
            json!({
                "query": "mutation { ...A @include(if: false) ...A } \
                          fragment A on Mutation { maybeWrite(fail: false) }",
            }),
            json!({ "maybeWrite": "block" }),
        ),
        (
            json!({
                "query": "mutation { ...A maybeWrite(fail: false) } \
                          fragment A on Mutation { maybeWrite(fail: false) @skip(if: true) }",
            }),
            json!({ "maybeWrite": "block" }),
        ),
    ] {
        let (app, journal) = boot().await;
        let reply = post(&app, body.clone()).await;
        assert_eq!(reply["data"], data, "{body}: {reply}");
        assert!(reply.get("errors").is_none(), "{body}: {reply}");
        assert_eq!(
            journal.read(),
            [
                "open graphql.operation per_unit",
                "block released",
                "settle graphql.operation true",
            ],
            "{body}: one root field, one block"
        );
    }
}

/// A selection a directive leaves nothing to answer, beside one that answers,
/// below a mutation's root: nothing runs, as for a repeated root selection.
#[tokio::test]
async fn a_mutation_hiding_an_answer_below_its_root_runs_nothing() {
    let logs = LogCapture::install();
    let (app, journal) = boot().await;
    let reply = post(
        &app,
        json!({ "query": "mutation { maybeShaped { inner { fine @skip(if: true) } inner { fine } } }" }),
    )
    .await;
    assert_eq!(reply["data"], Value::Null, "{reply}");
    assert_eq!(
        reply["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
        "{reply}"
    );
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "settle graphql.operation false"
        ],
    );
    assert!(dispatched(&logs).is_empty(), "no root field resolves");
}

/// A mutation whose root spreads a chain of `links` fragments, the last selecting
/// `last`.
fn fragment_chain(links: usize, last: &str) -> String {
    let mut query = String::from("mutation { ...F0 }");
    for link in 0..links - 1 {
        query.push_str(&format!(
            " fragment F{link} on Mutation {{ ...F{} }}",
            link + 1
        ));
    }
    query.push_str(&format!(
        " fragment F{} on Mutation {{ {last} }}",
        links - 1
    ));
    query
}

/// The read of a mutation's root runs before async-graphql's own recursion
/// limit, and a stack overflow aborts the process: as long a chain as a request's
/// body holds is the limit's to refuse.
#[tokio::test]
async fn a_fragment_chain_past_the_recursion_limit_is_refused_and_overflows_nothing() {
    let (app, journal) = boot().await;
    let deep = fragment_chain(6_000, "maybeWrite(fail: false)");
    let reply = post(&app, json!({ "query": deep })).await;
    assert_eq!(reply["data"], Value::Null);
    assert_eq!(
        reply["errors"][0]["message"],
        "The recursion depth of the query cannot be greater than `32`",
        "async-graphql says it, so a read before it may stop there"
    );
    assert!(journal.read().iter().all(|line| !line.contains("block")));
}

/// A chain as deep as async-graphql runs is read to its end, and the first one
/// deeper is refused by it: a bump that moves the limit fails here, and
/// `RECURSIVE_DEPTH` moves with it.
#[tokio::test]
async fn the_read_of_a_mutations_root_reaches_as_deep_as_a_document_runs() {
    let (app, _) = boot().await;
    let repeat = "maybeWrite(fail: false) maybeWrite(fail: false)";
    let reply = post(&app, json!({ "query": fragment_chain(32, repeat) })).await;
    assert_eq!(
        reply["errors"][0]["message"],
        r#"the mutation selects "maybeWrite" more than once at its root"#,
        "{reply}"
    );
    let reply = post(&app, json!({ "query": fragment_chain(33, repeat) })).await;
    assert_eq!(
        reply["errors"][0]["message"],
        "The recursion depth of the query cannot be greater than `32`",
        "{reply}"
    );
}

/// `@skip` and `@include` read a variable's default as an argument reads it,
/// and what the request sent over it.
#[tokio::test]
async fn a_directive_reads_a_variables_default() {
    let (app, _) = boot().await;
    for (body, data) in [
        (
            json!({ "query": "query($x: Boolean = true) { policy @include(if: $x) }" }),
            json!({ "policy": "pool" }),
        ),
        (
            json!({ "query": "query($x: Boolean = false) { policy @skip(if: $x) }" }),
            json!({ "policy": "pool" }),
        ),
        (
            json!({ "query": "query($x: Boolean = true) { policy a: policy @skip(if: $x) }" }),
            json!({ "policy": "pool" }),
        ),
        (
            json!({
                "query": "query($x: Boolean = true) { policy a: policy @include(if: $x) }",
                "variables": { "x": false },
            }),
            json!({ "policy": "pool" }),
        ),
        (
            json!({
                "query": "query($x: Boolean = false) { policy @include(if: $x) a: policy }",
                "variables": { "x": true },
            }),
            json!({ "policy": "pool", "a": "pool" }),
        ),
        (
            json!({
                "query": "query($x: Boolean = true) { ...F } \
                          fragment F on Query { policy @include(if: $x) }",
            }),
            json!({ "policy": "pool" }),
        ),
        (
            json!({
                "query": "query A($x: Boolean = false) { policy @include(if: $x) a: policy } \
                          query B($x: Boolean = true) { policy @include(if: $x) }",
                "operationName": "B",
            }),
            json!({ "policy": "pool" }),
        ),
    ] {
        let reply = post(&app, body.clone()).await;
        assert_eq!(reply["data"], data, "{body}: {reply}");
        assert!(reply.get("errors").is_none(), "{body}: {reply}");
    }
}

/// Each operation of a batch is read by its own variables and its own defaults,
/// and a refusal stays its own.
#[tokio::test]
async fn each_operation_of_a_batch_reads_its_own_variables() {
    let (app, _) = boot().await;
    let replies = post(
        &app,
        json!([
            { "query": "query($x: Boolean = true) { policy @include(if: $x) }" },
            {
                "query": "query($x: Boolean = true) { policy @include(if: $x) a: policy }",
                "variables": { "x": false },
            },
            { "query": "query($x: Boolean!) { policy @include(if: $x) }" },
            {
                "query": "mutation($x: Boolean = true) { \
                          maybeWrite(fail: false) @skip(if: $x) maybeWrite(fail: false) }",
            },
        ]),
    )
    .await;
    assert_eq!(replies[0]["data"], json!({ "policy": "pool" }), "{replies}");
    assert_eq!(replies[1]["data"], json!({ "a": "pool" }), "{replies}");
    assert_eq!(replies[2]["data"], Value::Null, "{replies}");
    assert_eq!(
        replies[2]["errors"][0]["message"],
        r#"Variable "$x" of required type "Boolean!" was not provided."#,
        "{replies}"
    );
    assert_eq!(
        replies[3]["data"],
        json!({ "maybeWrite": "block" }),
        "{replies}"
    );
}

/// What every root selection a directive leaves out answers: an operation that
/// ran nothing and failed at nothing reports no error and an empty object.
#[tokio::test]
async fn an_operation_every_root_selection_of_which_is_left_out_answers_an_empty_object() {
    let (app, journal) = boot().await;
    for (query, settled) in [
        ("{ policy @skip(if: true) }", false),
        ("query($x: Boolean = true) { policy @skip(if: $x) }", false),
        ("mutation { write(fail: false) @include(if: false) }", true),
    ] {
        let reply = post(&app, json!({ "query": query })).await;
        assert_eq!(reply, json!({ "data": {} }), "{query}");
        if settled {
            assert_eq!(
                journal.read().last().map(String::as_str),
                Some("settle graphql.operation true"),
                "{query}: nothing ran, and its answer reports no failure"
            );
        }
    }
}

/// GraphQL §6.1.2, *CoerceVariableValues*: a Non-Null variable the request
/// leaves absent, with no default, is a request error, so nothing runs —
/// neither a write a directive would have kept, nor a root field before the one
/// reading it.
#[tokio::test]
async fn a_required_variable_left_absent_is_refused_before_anything_runs() {
    let logs = LogCapture::install();
    let (app, journal) = boot().await;
    let documents = [
        (
            "query($x: Boolean!) { policy @include(if: $x) }",
            "x",
            "Boolean!",
        ),
        (
            "query($x: Boolean!) { policy @skip(if: $x) }",
            "x",
            "Boolean!",
        ),
        (
            "mutation($x: Boolean!) { write(fail: false) @skip(if: $x) }",
            "x",
            "Boolean!",
        ),
        (
            "mutation($fail: Boolean!) { first: write(fail: false) second: write(fail: $fail) }",
            "fail",
            "Boolean!",
        ),
    ];
    for (query, name, ty) in documents {
        let reply = post(&app, json!({ "query": query })).await;
        assert_eq!(reply["data"], Value::Null, "{query}: {reply}");
        assert_eq!(reply["errors"].as_array().map(Vec::len), Some(1), "{reply}");
        assert_eq!(
            reply["errors"][0]["message"],
            format!(r#"Variable "${name}" of required type "{ty}" was not provided."#),
            "{query}: {reply}"
        );
        assert_eq!(
            reply["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
            "{query}: {reply}"
        );
    }
    assert!(dispatched(&logs).is_empty(), "no root field resolves");
    assert!(
        !journal.read().iter().any(|entry| entry.contains("block")),
        "no root field opens a block: {:?}",
        journal.read()
    );

    let reply = post(
        &app,
        json!({
            "query": "query($x: Boolean!) { policy @include(if: $x) unknownField }",
        }),
    )
    .await;
    assert_eq!(
        reply["errors"][0]["message"], r#"Unknown field "unknownField" on type "Query"."#,
        "validation speaks first: {reply}"
    );
}

/// A nullable variable left absent is no refusal: it reads `null`.
#[tokio::test]
async fn a_nullable_variable_left_absent_is_no_refusal() {
    let (app, _) = boot().await;
    let reply = post(
        &app,
        json!({ "query": "query($hint: String) { hinted(hint: $hint) }" }),
    )
    .await;
    assert_eq!(reply["data"], json!({ "hinted": false }), "{reply}");
}

/// What a required variable sent as `null` reads, which §6.1.2 refuses too and
/// async-graphql (`7.2.1`) does not: a directive's `if` is refused by validation,
/// and a nullable argument reads the `null` the client wrote. Nothing runs that
/// the client did not ask for, so it stands.
#[tokio::test]
async fn a_required_variable_sent_null_is_refused_only_where_its_position_refuses_it() {
    let (app, _) = boot().await;
    let argument = post(
        &app,
        json!({
            "query": "query($hint: String!) { hinted(hint: $hint) }",
            "variables": { "hint": null },
        }),
    )
    .await;
    assert_eq!(argument["data"], json!({ "hinted": false }), "{argument}");
    let directive = post(
        &app,
        json!({
            "query": "query($x: Boolean!) { policy @skip(if: $x) }",
            "variables": { "x": null },
        }),
    )
    .await;
    assert_eq!(directive["data"], Value::Null, "{directive}");
    assert!(directive["errors"].is_array(), "{directive}");
}

#[tokio::test]
async fn a_batch_refuses_only_its_mutation_selecting_a_root_field_twice() {
    let (app, journal) = boot().await;
    let replies = post(
        &app,
        json!([
            { "query": "mutation { a: write(fail: false) a: write(fail: false) }" },
            { "query": "mutation { write(fail: false) }" },
        ]),
    )
    .await;
    assert_eq!(replies[0]["data"], Value::Null, "{replies}");
    assert_eq!(
        replies[0]["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
        "{replies}"
    );
    assert_eq!(replies[1]["data"]["write"], "block", "{replies}");
    assert_eq!(
        journal.read(),
        [
            "open graphql.operation per_unit",
            "settle graphql.operation false",
            "open graphql.operation per_unit",
            "block released",
            "settle graphql.operation true",
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

/// A failure the caller's own document earned fails nothing on the server's
/// span, as HTTP's `4xx` does: its line files `error`, and the span records
/// neither `error.type` nor a status. A failure of the server's own — a `5xx`'s
/// class, whatever its code, on a root field or below one — records both.
#[tokio::test]
async fn an_operation_records_an_error_on_its_span_only_for_a_failure_of_the_servers_own() {
    let logs = LogCapture::install();
    let (app, _) = boot().await;
    for document in [
        "query Unknown { noSuchField }",
        "mutation Refused { b: write(fail: true) }",
        "query Conflict { conflict }",
        "query NestedRefused { shape { broken } }",
        "query Failed { failed }",
        "query Storage { storage }",
        "query NestedFailed { shape { failedBelow } }",
        "query Mixed { conflict failed }",
    ] {
        post(&app, json!({ "query": document })).await;
    }
    post(&app, json!({ "query": "{" })).await;

    let lines = operation_lines(&logs);
    assert_eq!(lines.len(), 9, "{lines:#?}");
    for line in &lines {
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
            "whoever earned it, the operation answered an error: {line:#?}",
        );
    }
    for name in ["Unknown", "Refused", "Conflict", "NestedRefused"] {
        let span = operation_span(&logs, name);
        assert_eq!(span.field("error.type"), None, "{name}: {:?}", span.fields);
        assert_eq!(
            span.field("otel.status_code"),
            None,
            "{name}: {:?}",
            span.fields
        );
    }
    let no_document = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_graphql::unit::OPERATION.name()
                && span.field("graphql.operation.name").is_none()
        })
        .expect("the span of a document that does not parse");
    assert_eq!(
        no_document.field("error.type"),
        None,
        "{:?}",
        no_document.fields
    );
    for name in ["Failed", "Storage", "NestedFailed", "Mixed"] {
        assert_query_span_failed(&logs, name, nest_rs_core::operation_log::ERROR);
    }
}

/// What the unit could not honour is the server's own failure: nothing the
/// caller sent earned it.
#[tokio::test]
async fn an_operation_its_unit_could_not_honour_records_an_error_on_its_span() {
    let logs = LogCapture::install();
    let (app, _) = boot_with::<ProbeModule>(Settles::Unhonoured).await;
    post(
        &app,
        json!({ "query": "mutation Unhonoured { write(fail: false) }" }),
    )
    .await;

    let span = operation_span(&logs, "Unhonoured");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "{:?}",
        span.fields
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
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

/// The socket executes through `execute_stream`: what a POST reads of a
/// document, it reads.
#[tokio::test]
async fn over_graphql_ws_a_document_is_read_as_a_post_reads_it() {
    let (app, _) = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    for (index, (query, data, message)) in [
        (
            "query($x: Boolean = true) { policy @include(if: $x) }",
            json!({ "policy": "pool" }),
            None,
        ),
        (
            "query($x: Boolean!) { policy @include(if: $x) }",
            Value::Null,
            Some(r#"Variable "$x" of required type "Boolean!" was not provided."#),
        ),
        (
            "mutation { a: write(fail: false) a: write(fail: false) unknownField }",
            Value::Null,
            Some(r#"Unknown field "unknownField" on type "Mutation"."#),
        ),
        (
            "mutation { maybeWrite(fail: false) @skip(if: true) maybeWrite(fail: false) }",
            json!({ "maybeWrite": "block" }),
            None,
        ),
        (
            "mutation { shaped { inner { fine @skip(if: true) } inner { fine } } }",
            Value::Null,
            Some(
                r#"the operation selects "inner" more than once at one place, and its directives leave one of those selections nothing to answer: put the directive on that selection, to leave it out whole"#,
            ),
        ),
        (
            "mutation { shaped { fine @skip(if: true) } }",
            json!({ "shaped": {} }),
            None,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let id = index.to_string();
        socket.subscribe(&id, query);
        let item = socket.next_item(&id).await.expect("the operation answers");
        assert_eq!(item["data"], data, "{query}: {item}");
        assert_eq!(
            item["errors"][0]["message"].as_str(),
            message,
            "{query}: {item}"
        );
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
    let query = "mutation { a: write(fail: false) a: write(fail: false) }";
    socket
        .send_text(
            json!({ "id": "3", "type": "subscribe", "payload": { "query": query } }).to_string(),
        )
        .await;
    let refused = next_message(&mut socket).await;
    assert_eq!(refused["type"], "next", "{refused}");
    assert_eq!(refused["payload"]["data"], Value::Null, "{refused}");
    assert_eq!(
        refused["payload"]["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
        "{refused}"
    );
    assert_eq!(next_message(&mut socket).await["type"], "complete");

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
            "open graphql.operation per_unit",
            "settle graphql.operation false",
        ],
        "the socket's pool unit holds no transaction; each mutation opens and settles its own, \
         on its own answer, and one selecting a root field twice runs none"
    );
}
