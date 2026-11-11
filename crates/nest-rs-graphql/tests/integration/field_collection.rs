//! Covers `src/field_collection.rs`: a document async-graphql 7.2.1 would answer
//! otherwise than GraphQL (October 2021) §6.3.2 collects it is rewritten before
//! validation into one it answers as the specification reads — a mutation's root
//! field selected twice runs once, the copies of a list of lists merge, and a type
//! condition on a union applies wherever a member of it is resolved — and the
//! rewritten document passes every check the one sent would have.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_config::Surface;
use nest_rs_core::{Layer, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::{Interface, Result, SimpleObject, Union};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};

/// How many writes the mutation root ran, in this test's process.
static WRITES: AtomicUsize = AtomicUsize::new(0);

/// A cell of a grid: `grid` nests a list in a list, `broken` fails, `vault` holds a
/// field its own guard refuses.
#[derive(SimpleObject)]
#[graphql(complex)]
struct Cell {
    fine: i32,
}

/// What [`SealedResolver`]'s guard stands in front of.
#[derive(SimpleObject)]
#[graphql(complex)]
struct Vault {
    id: i32,
}

/// The other member of [`Either`], a [`Node`] too.
#[derive(SimpleObject)]
struct Leaf {
    fine: i32,
    leaf: i32,
}

/// A [`Node`] no union holds.
#[derive(SimpleObject)]
struct Stone {
    fine: i32,
    stone: i32,
}

/// A union of a cell and a leaf.
#[derive(Union)]
enum Either {
    Cell(Cell),
    Leaf(Leaf),
}

/// An interface every object here implements, one of which [`Either`] holds not.
#[derive(Interface)]
#[graphql(field(name = "fine", ty = "&i32"))]
enum Node {
    Cell(Cell),
    Leaf(Leaf),
    Stone(Stone),
}

fn cell() -> Cell {
    Cell { fine: 1 }
}

fn leaf() -> Leaf {
    Leaf { fine: 2, leaf: 2 }
}

fn stone() -> Stone {
    Stone { fine: 3, stone: 3 }
}

#[resolver]
struct CellResolver;

#[operations]
impl CellResolver {
    #[query]
    #[public]
    async fn grid(&self) -> Vec<Vec<Cell>> {
        vec![vec![cell(), cell()], vec![cell()]]
    }

    #[query]
    #[public]
    async fn cell(&self) -> Option<Cell> {
        Some(cell())
    }

    #[query]
    #[public]
    async fn either(&self) -> Option<Either> {
        Some(Either::Cell(cell()))
    }

    #[query]
    #[public]
    async fn eithers(&self) -> Vec<Either> {
        vec![Either::Cell(cell()), Either::Leaf(leaf())]
    }

    #[query]
    #[public]
    async fn nodes(&self) -> Vec<Node> {
        vec![Node::Cell(cell()), Node::Leaf(leaf()), Node::Stone(stone())]
    }

    #[mutation]
    #[public]
    async fn write(&self, fail: bool) -> Result<i32> {
        tokio::task::yield_now().await;
        let written = WRITES.fetch_add(1, Ordering::SeqCst) + 1;
        match fail {
            true => Err("the write was refused".into()),
            false => Ok(i32::try_from(written).expect("a few writes")),
        }
    }

    #[mutation]
    #[public]
    async fn stamp(&self) -> Cell {
        WRITES.fetch_add(1, Ordering::SeqCst);
        cell()
    }
}

/// What a [`Cell`] answers below it.
#[resolver]
struct CellFieldsResolver;

#[operations]
impl CellFieldsResolver {
    #[field_resolver]
    async fn grid(&self, parent: &Cell) -> Vec<Vec<Cell>> {
        vec![vec![Cell { fine: parent.fine }]]
    }

    #[field_resolver]
    async fn inner(&self, parent: &Cell) -> Option<Cell> {
        Some(Cell { fine: parent.fine })
    }

    #[field_resolver]
    async fn broken(&self, _parent: &Cell) -> Result<Option<i32>> {
        Err("the nullable field failed".into())
    }

    #[field_resolver]
    async fn vault(&self, parent: &Cell) -> Vault {
        Vault { id: parent.fine }
    }
}

/// Refuses every GraphQL operation it is asked about.
#[injectable]
#[derive(Default, Layer)]
struct DenyAll;

#[async_trait]
impl Guard for DenyAll {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("denied by the field's guard"))
    }
}

impl GuardFor<nest_rs_graphql::GraphqlView> for DenyAll {}
impl GuardFor<nest_rs_http::HttpView> for DenyAll {}

#[resolver]
#[use_guards(DenyAll)]
struct SealedResolver;

#[operations]
impl SealedResolver {
    #[field_resolver]
    async fn sealed(&self, parent: &Vault) -> Option<i32> {
        Some(parent.id)
    }
}

#[module(imports = [GraphqlModule], providers = [DenyAll, CellResolver, CellFieldsResolver, SealedResolver])]
struct CellModule;

#[module(
    imports = [
        GraphqlModule::for_root(GraphqlConfig {
            max_complexity: Some(3),
            max_depth: Some(2),
            ..GraphqlConfig::default()
        }),
        CellModule,
    ],
)]
struct LimitedModule;

#[module(
    imports = [
        GraphqlModule::for_root(GraphqlConfig {
            introspection: Surface::Closed,
            ..GraphqlConfig::default()
        }),
        CellModule,
    ],
)]
struct ClosedModule;

async fn boot<M: nest_rs_core::Module + 'static>() -> TestApp {
    TestApp::builder()
        .module::<M>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn post(app: &TestApp, body: &Value) -> Value {
    let resp = app.http().post("/graphql").body_json(body).send().await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

async fn query(app: &TestApp, document: &str) -> Value {
    post(app, &json!({ "query": document })).await
}

/// The writes the root ran since `before`.
fn written_since(before: usize) -> usize {
    WRITES.load(Ordering::SeqCst) - before
}

/// What a copy selecting otherwise than the first, by its field or its arguments,
/// is refused with.
fn otherwise(key: &str) -> String {
    format!(
        r#"the operation selects "{key}" more than once at one place with different fields or arguments"#
    )
}

/// GraphQL §6.3.2 collects one response key once at a mutation's root, through
/// fragments: async-graphql 7.2.1 ran each copy's write, so each copy now merges
/// into the first, what each selects below it included, and the write runs once.
#[tokio::test]
async fn a_mutation_selecting_one_root_field_twice_runs_it_once() {
    let app = boot::<CellModule>().await;
    for (document, key) in [
        (
            "mutation { a: write(fail: false) a: write(fail: false) }",
            "a",
        ),
        (
            "mutation { write(fail: false) ... on Mutation { write(fail: false) } }",
            "write",
        ),
        (
            "mutation { ...A ...B } \
             fragment A on Mutation { w: write(fail: false) } \
             fragment B on Mutation { w: write(fail: false) }",
            "w",
        ),
        (
            "mutation { ...A ...A } fragment A on Mutation { write(fail: false) }",
            "write",
        ),
        (
            "mutation { ...A ... { ...A } } fragment A on Mutation { write(fail: false) }",
            "write",
        ),
    ] {
        let before = WRITES.load(Ordering::SeqCst);
        let answer = query(&app, document).await;
        assert_eq!(written_since(before), 1, "{document}: {answer}");
        let written = i32::try_from(before + 1).expect("a few writes");
        assert_eq!(answer, json!({ "data": { key: written } }), "{document}");
    }
    let before = WRITES.load(Ordering::SeqCst);
    let answer = query(
        &app,
        "mutation { s: stamp { fine } s: stamp { inner { fine } broken } }",
    )
    .await;
    assert_eq!(written_since(before), 1, "{answer}");
    assert_eq!(
        answer["data"],
        json!({ "s": { "fine": 1, "inner": { "fine": 1 }, "broken": null } }),
        "{answer}"
    );
    assert_eq!(
        answer["errors"][0]["path"],
        json!(["s", "broken"]),
        "{answer}"
    );
}

/// Copies naming different fields or arguments are no one field, which §5.3.2
/// refuses and async-graphql 7.2.1's validation passes where a type condition
/// stands between them: refused before any runs, at the copy that differs.
#[tokio::test]
async fn a_mutation_selecting_one_key_twice_otherwise_is_refused_before_any_runs() {
    let app = boot::<CellModule>().await;
    for document in [
        "mutation { a: write(fail: false) ... on Mutation { a: write(fail: true) } }",
        "mutation { a: write(fail: false) ... on Mutation { a: stamp { fine } } }",
        "mutation { ...A a: write(fail: false) } fragment A on Mutation { a: write(fail: true) }",
    ] {
        let before = WRITES.load(Ordering::SeqCst);
        let answer = query(&app, document).await;
        assert_eq!(written_since(before), 0, "{document}: {answer}");
        assert_eq!(answer["data"], Value::Null, "{document}: {answer}");
        assert_eq!(answer["errors"][0]["message"], otherwise("a"), "{document}");
        assert_eq!(
            answer["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
            "{document}: {answer}"
        );
    }
}

/// The cells of `grid`, each answering `cell`.
fn grid_of(cell: &Value) -> Value {
    json!([[cell, cell], [cell]])
}

/// async-graphql 7.2.1 joins a later copy of one response key into the first only
/// where the first's list items are objects (`insert_value`), so a list of lists
/// selected twice at one place answered one copy's cells: the copies now merge, at
/// that place and below copies of its parent, on `POST`, in a batch and over the
/// socket.
#[tokio::test]
async fn a_list_of_lists_selected_twice_answers_what_each_copy_selects() {
    let app = boot::<CellModule>().await;
    let merged = json!({ "fine": 1, "inner": { "fine": 1 } });
    for (document, data) in [
        (
            "{ grid { fine } grid { inner { fine } } }",
            json!({ "grid": grid_of(&merged) }),
        ),
        (
            "{ grid { inner { fine } } grid { fine } }",
            json!({ "grid": grid_of(&json!({ "inner": { "fine": 1 }, "fine": 1 })) }),
        ),
        (
            "{ grid { fine } ... { grid { inner { fine } } } }",
            json!({ "grid": grid_of(&merged) }),
        ),
        (
            "{ ...A ...B } fragment A on Query { grid { fine } } \
             fragment B on Query { grid { inner { fine } } }",
            json!({ "grid": grid_of(&merged) }),
        ),
        (
            "{ cell { grid { fine } } cell { grid { inner { fine } } } }",
            json!({ "cell": { "grid": [[merged.clone()]] } }),
        ),
        (
            "{ cell { inner { grid { fine } } } cell { inner { grid { inner { fine } } } } }",
            json!({ "cell": { "inner": { "grid": [[merged.clone()]] } } }),
        ),
        (
            "{ either { ... on Cell { grid { fine } } } \
               either { ... on Cell { grid { inner { fine } } } } }",
            json!({ "either": { "grid": [[merged.clone()]] } }),
        ),
        (
            "{ grid { grid { fine } } grid { grid { inner { fine } } } }",
            json!({ "grid": grid_of(&json!({ "grid": [[merged.clone()]] })) }),
        ),
    ] {
        let answer = query(&app, document).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
    let answer = query(&app, "{ g: grid { fine } g: grid { broken } }").await;
    assert_eq!(
        answer["data"],
        json!({ "g": grid_of(&json!({ "fine": 1, "broken": null })) }),
        "{answer}"
    );
    assert_eq!(
        answer["errors"].as_array().map(Vec::len),
        Some(3),
        "one per cell: {answer}"
    );

    let document = "{ grid { fine } grid { inner { fine } } }";
    let answer = post(
        &app,
        &json!([{ "query": document }, { "query": "{ grid { fine } }" }]),
    )
    .await;
    assert_eq!(answer[0], json!({ "data": { "grid": grid_of(&merged) } }));
    assert_eq!(
        answer[1],
        json!({ "data": { "grid": grid_of(&json!({ "fine": 1 })) } })
    );
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("grid", document);
    let item = socket.next_item("grid").await.expect("the answer");
    assert_eq!(item, json!({ "data": { "grid": grid_of(&merged) } }));
}

/// Copies of a list of lists naming different fields or arguments are refused, as at
/// a mutation's root: no one field answers them.
#[tokio::test]
async fn a_list_of_lists_selected_twice_otherwise_is_refused() {
    let app = boot::<CellModule>().await;
    let answer = query(
        &app,
        "{ g: grid { fine } ... on Query { g: eithers { __typename } } }",
    )
    .await;
    assert_eq!(answer["data"], Value::Null, "{answer}");
    assert_eq!(answer["errors"][0]["message"], otherwise("g"), "{answer}");
}

/// Copies of one key at one place naming different fields or arguments are no one
/// field (§5.3.2), wherever they stand: async-graphql 7.2.1's validation compares
/// copies under one type condition alone, and answered two fields under one key.
#[tokio::test]
async fn copies_naming_different_fields_under_different_conditions_are_refused() {
    let app = boot::<CellModule>().await;
    for (document, key) in [
        (
            "{ a: cell { fine } ... on Query { a: either { __typename } } }",
            "a",
        ),
        (
            "{ cell { a: fine ... on Cell { a: inner { fine } } } }",
            "a",
        ),
        (
            "{ nodes { ... on Node { x: fine } ... on Cell { x: broken } } }",
            "x",
        ),
    ] {
        let answer = query(&app, document).await;
        assert_eq!(answer["data"], Value::Null, "{document}: {answer}");
        assert_eq!(answer["errors"][0]["message"], otherwise(key), "{document}");
    }
    let answer = query(
        &app,
        "{ eithers { ... on Cell { x: fine } ... on Leaf { x: leaf } } }",
    )
    .await;
    assert_eq!(
        answer,
        json!({ "data": { "eithers": [{ "x": 1 }, { "x": 2 }] } }),
        "copies no one value meets name what they will"
    );
}

/// GraphQL §6.3.2 *DoesFragmentTypeApply* applies a condition on a union to a value
/// of any member of it, wherever it is resolved, where async-graphql 7.2.1
/// (`Fields::add_set`) applies it only where that union is the type resolved.
#[tokio::test]
async fn a_condition_on_a_union_applies_wherever_a_member_is_resolved() {
    let app = boot::<CellModule>().await;
    for (document, data) in [
        (
            "{ cell { ... on Either { ... on Cell { fine } } } }",
            json!({ "cell": { "fine": 1 } }),
        ),
        (
            "{ cell { ...E } } fragment E on Either { ... on Cell { fine } ... on Leaf { leaf } }",
            json!({ "cell": { "fine": 1 } }),
        ),
        (
            "{ cell { ... on Either { __typename } } }",
            json!({ "cell": { "__typename": "Cell" } }),
        ),
        (
            "{ either { ... on Cell { ... on Either { ... on Cell { fine } } } } }",
            json!({ "either": { "fine": 1 } }),
        ),
        (
            "{ either { ... on Either { ... on Cell { fine } } } }",
            json!({ "either": { "fine": 1 } }),
        ),
        (
            "{ nodes { ... on Either { __typename ... on Node { fine } } } }",
            json!({ "nodes": [
                { "__typename": "Cell", "fine": 1 },
                { "__typename": "Leaf", "fine": 2 },
                {},
            ] }),
        ),
        (
            "{ eithers { ... on Node { ... on Either { ... on Leaf { leaf } } } } }",
            json!({ "eithers": [{}, { "leaf": 2 }] }),
        ),
        (
            "{ grid { ... on Either { ... on Cell { fine } } } }",
            json!({ "grid": grid_of(&json!({ "fine": 1 })) }),
        ),
        (
            "{ grid { fine } grid { ... on Either { ... on Cell { inner { fine } } } } }",
            json!({ "grid": grid_of(&json!({ "fine": 1, "inner": { "fine": 1 } })) }),
        ),
    ] {
        let answer = query(&app, document).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
}

/// A rewritten document is judged as the one sent: what a merge moves, what a
/// rewrite of a condition leaves out and what a fragment held are validated where
/// the client wrote them, each error said once.
#[tokio::test]
async fn a_rewritten_document_says_what_validation_says_of_the_one_sent() {
    let app = boot::<CellModule>().await;
    for (document, said) in [
        (
            "mutation { a: stamp { fine } a: stamp { unknownField } }",
            r#"Unknown field "unknownField" on type "Cell"."#,
        ),
        (
            "{ grid { fine } grid { inner { unknownField } } }",
            r#"Unknown field "unknownField" on type "Cell"."#,
        ),
        (
            "{ cell { ... on Either { ... on Leaf { unknownField } ... on Cell { fine } } } }",
            r#"Unknown field "unknownField" on type "Leaf"."#,
        ),
        (
            "{ cell { ... on Either { fine } } }",
            r#"Unknown field "fine" on type "Either"."#,
        ),
        (
            "{ cell { ...E } c: cell { ...E } } fragment E on Either { ... on Cell { unknownField } }",
            r#"Unknown field "unknownField" on type "Cell"."#,
        ),
        (
            "{ cell { ...E } } fragment E on Either { ... on Cell { fine } } \
             fragment U on Cell { fine }",
            r#"Fragment "U" is never used"#,
        ),
        (
            "mutation { a: write(fail: false) a: write(fail: false) unknownField }",
            r#"Unknown field "unknownField" on type "Mutation"."#,
        ),
        (
            "mutation { a: stamp { fine } a: stamp }",
            r#"Field "stamp" of type "Cell" must have a selection of subfields"#,
        ),
    ] {
        let answer = query(&app, document).await;
        assert_eq!(answer["data"], Value::Null, "{document}: {answer}");
        let said_back: Vec<&str> = answer["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|error| error["message"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(said_back, [said], "{document}: {answer}");
    }
    let answer = query(
        &app,
        "{ cell { ...E } } fragment E on Either { ... on Cell { ...E } }",
    )
    .await;
    assert_eq!(answer["data"], Value::Null, "{answer}");
    assert_eq!(
        answer["errors"][0]["message"],
        "The recursion depth of the query cannot be greater than `32`",
        "a cycle is refused as sent: {answer}"
    );
}

/// What a rewrite moves or leaves out keeps the variables it reads: no variable is
/// left unused that the document sent reads.
#[tokio::test]
async fn a_variable_read_where_a_rewrite_moves_is_still_read() {
    let app = boot::<CellModule>().await;
    for (document, data) in [
        (
            "query($x: Boolean!) { cell { ... on Either { \
               ... on Leaf { leaf @include(if: $x) } ... on Cell { fine } } } }",
            json!({ "cell": { "fine": 1 } }),
        ),
        (
            "query($x: Boolean!) { grid { fine } grid @include(if: $x) { inner { fine } } }",
            json!({ "grid": grid_of(&json!({ "fine": 1, "inner": { "fine": 1 } })) }),
        ),
        (
            "mutation($x: Boolean!) { a: stamp { fine } a: stamp @include(if: $x) { fine } }",
            json!({ "a": { "fine": 1 } }),
        ),
    ] {
        let answer = post(
            &app,
            &json!({ "query": document, "variables": { "x": true } }),
        )
        .await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
}

/// Complexity and depth are counted on the document that runs: copies merged count
/// once, as they run once, and what a merge moves stands as deep as it was written.
#[tokio::test]
async fn the_limits_count_what_runs() {
    let app = boot::<LimitedModule>().await;
    let answer = query(&app, "mutation { a: stamp { fine } a: stamp { fine } }").await;
    assert_eq!(
        answer,
        json!({ "data": { "a": { "fine": 1 } } }),
        "one write and its field, three in all"
    );
    let answer = query(
        &app,
        "mutation { a: stamp { fine } a: stamp { inner { fine } } }",
    )
    .await;
    assert_eq!(answer["data"], Value::Null, "{answer}");
    assert_eq!(answer["errors"][0]["message"], "Query is too complex.");
    let answer = query(
        &app,
        "{ cell { fine } c: cell { ... on Either { ... on Cell { fine } } } }",
    )
    .await;
    assert_eq!(answer["errors"][0]["message"], "Query is too complex.");
    let answer = query(
        &app,
        "{ g: cell { ... on Either { ... on Cell { inner { fine } } } } }",
    )
    .await;
    assert_eq!(answer["data"], Value::Null, "{answer}");
    assert_eq!(answer["errors"][0]["message"], "Query is nested too deep.");
}

/// A field's guard runs on every row a rewrite makes it resolve.
#[tokio::test]
async fn a_field_a_rewrite_resolves_runs_its_guard() {
    let app = boot::<CellModule>().await;
    for (document, refused) in [
        ("{ grid { fine } grid { vault { sealed } } }", 3),
        (
            "{ cell { ... on Either { ... on Cell { vault { sealed } } } } }",
            1,
        ),
        (
            "mutation { a: stamp { fine } a: stamp { vault { sealed } } }",
            1,
        ),
    ] {
        let answer = query(&app, document).await;
        let errors = answer["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(errors.len(), refused, "{document}: {answer}");
        for error in errors {
            assert_eq!(
                error["extensions"]["code"], "FORBIDDEN",
                "{document}: {answer}"
            );
        }
    }
}

/// Introspection a deployment closes stays closed inside a rewritten document.
#[tokio::test]
async fn closed_introspection_stays_closed_in_a_rewritten_document() {
    let app = boot::<ClosedModule>().await;
    let plain = query(&app, r#"{ __type(name: "Cell") { name } }"#).await;
    assert_eq!(plain, json!({ "data": { "__type": null } }), "{plain}");
    let rewritten = query(
        &app,
        r#"{ cell { ... on Either { ... on Cell { fine } } } __type(name: "Cell") { name } }"#,
    )
    .await;
    assert_eq!(
        rewritten,
        json!({ "data": { "cell": { "fine": 1 }, "__type": null } }),
        "{rewritten}"
    );
}
