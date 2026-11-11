//! Covers `src/null_propagation.rs`: a field that errs answers `null` beside
//! its error, and a Non-Null one nulls its nearest nullable parent — `data`
//! itself at the root (GraphQL, October 2021, §6.4.4) — on `POST`, in a batch
//! and over the socket.

use nest_rs_core::module;
use nest_rs_graphql::async_graphql::futures_util::stream as futures_stream;
use nest_rs_graphql::async_graphql::{Json, Object, Result, SimpleObject, Union};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};

/// An object whose fields fail by nullability: `broken: Int`, `strict: Int!`.
struct Shape;

#[Object]
impl Shape {
    async fn fine(&self) -> i32 {
        1
    }

    async fn broken(&self) -> Result<Option<i32>> {
        Err("the nullable field failed".into())
    }

    async fn strict(&self) -> Result<i32> {
        Err("the non-null field failed".into())
    }

    async fn inner(&self) -> Option<Shape> {
        Some(Shape)
    }

    async fn sure_inner(&self) -> Shape {
        Shape
    }

    async fn grid(&self) -> Vec<Vec<Shape>> {
        vec![vec![Shape]]
    }
}

/// The other member of [`Either`].
#[derive(SimpleObject)]
struct Other {
    other: i32,
    grid: Vec<Vec<Other>>,
}

/// What a type condition reads: a `Shape` or an `Other`.
#[derive(Union)]
enum Either {
    Shape(Shape),
    Other(Other),
}

#[resolver]
struct ShapeResolver;

#[operations]
impl ShapeResolver {
    #[query]
    #[public]
    async fn fine(&self) -> i32 {
        1
    }

    #[query]
    #[public]
    async fn broken(&self) -> Result<Option<i32>> {
        Err("the nullable root field failed".into())
    }

    #[query]
    #[public]
    async fn strict(&self) -> Result<i32> {
        Err("the non-null root field failed".into())
    }

    #[query]
    #[public]
    async fn shape(&self) -> Option<Shape> {
        Some(Shape)
    }

    #[query]
    #[public]
    async fn shapes(&self) -> Vec<Option<Shape>> {
        vec![Some(Shape), Some(Shape)]
    }

    #[query]
    #[public]
    async fn sure_shape(&self) -> Shape {
        Shape
    }

    #[query]
    #[public]
    async fn sure_shapes(&self) -> Vec<Shape> {
        vec![Shape, Shape]
    }

    #[query]
    #[public]
    async fn either(&self) -> Option<Either> {
        Some(Either::Shape(Shape))
    }

    #[query]
    #[public]
    async fn sure_either(&self) -> Either {
        Either::Shape(Shape)
    }

    #[query]
    #[public]
    async fn eithers(&self) -> Vec<Either> {
        vec![
            Either::Shape(Shape),
            Either::Other(Other {
                other: 2,
                grid: Vec::new(),
            }),
        ]
    }

    #[query]
    #[public]
    async fn grid(&self) -> Vec<Vec<Shape>> {
        vec![vec![Shape, Shape], vec![Shape]]
    }

    #[query]
    #[public]
    async fn meta(&self) -> Json<Value> {
        Json(meta())
    }

    #[mutation]
    #[public]
    async fn reshaped(&self) -> Option<Shape> {
        Some(Shape)
    }

    #[subscription]
    #[public]
    async fn shaped(&self) -> impl futures_stream::Stream<Item = Shape> {
        futures_stream::iter([Shape])
    }
}

/// What the `JSON` scalar `meta` answers: members named as the key given to an
/// object could be.
fn meta() -> Value {
    json!({
        "__nestrs_key": 1,
        "__nestrs_key1": 2,
        "kept": { "__nestrs_key": 3 },
    })
}

#[module(imports = [GraphqlModule], providers = [ShapeResolver])]
struct ShapeApp;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ShapeApp>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn post(app: &TestApp, body: &Value) -> Value {
    let resp = app.http().post("/graphql").body_json(body).send().await;
    resp.assert_status_is_ok();
    resp.json().await.value().deserialize()
}

/// The paths a response's errors carry, in order.
fn paths(answer: &Value) -> Vec<Value> {
    answer["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("errors: {answer}"))
        .iter()
        .map(|error| error["path"].clone())
        .collect()
}

/// Each document, the `data` it answers, and the paths of its errors.
fn cases() -> Vec<(&'static str, Value, Vec<Value>)> {
    vec![
        (
            "{ fine broken }",
            json!({ "fine": 1, "broken": null }),
            vec![json!(["broken"])],
        ),
        (
            "{ broken }",
            json!({ "broken": null }),
            vec![json!(["broken"])],
        ),
        (
            "{ shape { fine broken } }",
            json!({ "shape": { "fine": 1, "broken": null } }),
            vec![json!(["shape", "broken"])],
        ),
        (
            "{ shape { broken } }",
            json!({ "shape": { "broken": null } }),
            vec![json!(["shape", "broken"])],
        ),
        (
            "{ fine shape { fine strict } }",
            json!({ "fine": 1, "shape": null }),
            vec![json!(["shape", "strict"])],
        ),
        ("{ fine strict }", Value::Null, vec![json!(["strict"])]),
        (
            "{ shapes { fine strict } }",
            json!({ "shapes": [null, null] }),
            vec![
                json!(["shapes", 0, "strict"]),
                json!(["shapes", 1, "strict"]),
            ],
        ),
        (
            "{ a: shape { ...parts } b: shape { ... on Shape { s: strict } } c: fine } \
             fragment parts on Shape { x: broken }",
            json!({ "a": { "x": null }, "b": null, "c": 1 }),
            vec![json!(["a", "x"]), json!(["b", "s"])],
        ),
    ]
}

/// The key of a field that erred is present, `null`, never absent.
fn assert_answers(answer: &Value, document: &str, data: &Value, errors: &[Value]) {
    assert_eq!(&answer["data"], data, "{document}: {answer}");
    let mut found = paths(answer);
    found.sort_by_key(Value::to_string);
    assert_eq!(found, errors, "{document}: {answer}");
}

#[tokio::test]
async fn a_field_that_errs_answers_null_and_a_non_null_one_nulls_its_parent() {
    let app = boot().await;
    for (document, data, errors) in cases() {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_answers(&answer, document, &data, &errors);
    }
}

#[tokio::test]
async fn each_operation_of_a_batch_answers_its_own_nulls() {
    let app = boot().await;
    let answers = post(
        &app,
        &json!([{ "query": "{ fine broken }" }, { "query": "{ fine strict }" }]),
    )
    .await;
    assert_eq!(
        answers[0]["data"],
        json!({ "fine": 1, "broken": null }),
        "{answers}"
    );
    assert_eq!(answers[1]["data"], Value::Null, "{answers}");
    assert_eq!(
        answers[1]["errors"][0]["path"],
        json!(["strict"]),
        "{answers}"
    );
}

/// The socket executes through `execute_stream`.
#[tokio::test]
async fn an_operation_over_the_socket_answers_its_nulls() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    for (index, (document, data, errors)) in cases().into_iter().enumerate() {
        let id = index.to_string();
        socket.subscribe(&id, document);
        let item = socket.next_item(&id).await.expect("the operation answers");
        assert_answers(&item, document, &data, &errors);
    }
}

/// A copy a directive leaves out is no copy: the object answers as the one
/// that ran, its nulls placed, on `POST` and in a batch alike; both copies run
/// when both are kept, and the object answers what each selects.
#[tokio::test]
async fn a_copy_a_directive_leaves_out_hides_no_answer() {
    let app = boot().await;
    for (body, data) in [
        (
            json!({ "query": "{ shape { broken } shape @skip(if: true) { fine } }" }),
            json!({ "shape": { "broken": null } }),
        ),
        (
            json!({
                "query": "query($x: Boolean!) { shape { broken } shape @include(if: $x) { fine } }",
                "variables": { "x": false },
            }),
            json!({ "shape": { "broken": null } }),
        ),
        (
            json!({
                "query": "query($x: Boolean = true) { shape { broken } shape @skip(if: $x) { fine } }",
            }),
            json!({ "shape": { "broken": null } }),
        ),
        (
            json!({
                "query": "{ ...A ...B } fragment A on Query { shape { broken } } \
                          fragment B on Query { shape @skip(if: true) { fine } }",
            }),
            json!({ "shape": { "broken": null } }),
        ),
        (
            json!({
                "query": "{ shapes { broken } shapes @skip(if: true) { fine } }",
            }),
            json!({ "shapes": [{ "broken": null }, { "broken": null }] }),
        ),
        (
            json!({
                "query": "query($x: Boolean!) { shape { broken } shape @include(if: $x) { fine } }",
                "variables": { "x": true },
            }),
            json!({ "shape": { "broken": null, "fine": 1 } }),
        ),
    ] {
        let answer = post(&app, &body).await;
        assert_eq!(answer["data"], data, "{body}: {answer}");
    }
}

/// An object its directives leave nothing to answer is an object all the same:
/// GraphQL §6.3 answers `{}`, at a nullable position as at a Non-Null one, where
/// async-graphql (`7.2.1`, `do_resolve_container`) answers `null`.
#[tokio::test]
async fn a_nullable_object_every_selection_of_which_is_left_out_answers_an_empty_object() {
    let app = boot().await;
    for (document, data) in [
        ("{ shape { fine @skip(if: true) } }", json!({ "shape": {} })),
        (
            "{ shapes { fine @skip(if: true) } }",
            json!({ "shapes": [{}, {}] }),
        ),
        (
            "{ shape { fine @skip(if: true) } shape { broken @include(if: false) } }",
            json!({ "shape": {} }),
        ),
        (
            "{ shape { inner { fine @skip(if: true) } } }",
            json!({ "shape": { "inner": {} } }),
        ),
        (
            "{ shape { ...F } } fragment F on Shape { fine @skip(if: true) }",
            json!({ "shape": {} }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
}

/// A Non-Null position answers the same `{}`.
#[tokio::test]
async fn a_non_null_object_every_selection_of_which_is_left_out_answers_an_empty_object() {
    let app = boot().await;
    for (body, data) in [
        (
            json!({ "query": "{ sureShape { fine @skip(if: true) } }" }),
            json!({ "sureShape": {} }),
        ),
        (
            json!({ "query": "{ sureShapes { fine @skip(if: true) } }" }),
            json!({ "sureShapes": [{}, {}] }),
        ),
        (
            json!({
                "query": "query($x: Boolean = false) { sureShape { fine @include(if: $x) } }",
            }),
            json!({ "sureShape": {} }),
        ),
        (
            json!({
                "query": "query($x: Boolean!) { sureShape { fine @include(if: $x) } }",
                "variables": { "x": false },
            }),
            json!({ "sureShape": {} }),
        ),
        (
            json!({
                "query": "{ sureShape { ...A ... @skip(if: true) { fine } } } \
                          fragment A on Shape { fine @skip(if: true) }",
            }),
            json!({ "sureShape": {} }),
        ),
        (
            json!({ "query": "{ sureShape { sureInner { fine @skip(if: true) } fine } }" }),
            json!({ "sureShape": { "sureInner": {}, "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { sureInner { fine @skip(if: true) } } }" }),
            json!({ "shape": { "sureInner": {} } }),
        ),
        (
            json!({
                "query": "{ a: sureShape { fine @skip(if: true) } \
                          a: sureShape { broken @include(if: false) } }",
            }),
            json!({ "a": {} }),
        ),
    ] {
        let answer = post(&app, &body).await;
        assert_eq!(answer, json!({ "data": data }), "{body}");
    }
}

/// Beside a field's error too, its `null` placed as if nothing else stood there.
#[tokio::test]
async fn an_empty_object_stands_beside_a_field_error() {
    let app = boot().await;
    let answer = post(
        &app,
        &json!({ "query": "{ broken sureShape { fine @skip(if: true) } }" }),
    )
    .await;
    assert_eq!(
        answer["data"],
        json!({ "broken": null, "sureShape": {} }),
        "{answer}"
    );
    assert_eq!(paths(&answer), [json!(["broken"])]);
}

/// async-graphql resolves each selection of one response key apart and keeps
/// whichever finished first where one answered `null`, so a copy its directives,
/// a type condition or its every field's error left with no key hid what the
/// other selected, with no error. Each object keeps a key of its own, so none
/// does, whatever the order the copies are written in or finish in.
#[tokio::test]
async fn no_copy_hides_another_selections_answer() {
    let app = boot().await;
    for (body, data) in [
        (
            json!({ "query": "{ shape { fine @skip(if: true) } shape { fine } }" }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { fine } shape { fine @skip(if: true) } }" }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({
                "query": "query($x: Boolean = false) { \
                          shape { fine @include(if: $x) } shape { fine } }",
            }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({
                "query": "query($x: Boolean!) { shape { fine @include(if: $x) } shape { fine } }",
                "variables": { "x": false },
            }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ ... { shape { fine @skip(if: true) } } shape { fine } }" }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shapes { fine @skip(if: true) } shapes { fine } }" }),
            json!({ "shapes": [{ "fine": 1 }, { "fine": 1 }] }),
        ),
        (
            json!({ "query": "{ sureShape { fine @skip(if: true) } sureShape { fine } }" }),
            json!({ "sureShape": { "fine": 1 } }),
        ),
        (
            json!({
                "query": "{ ...A ...B } fragment A on Query { shape { ...C } } \
                          fragment B on Query { shape { fine } } \
                          fragment C on Shape { fine @skip(if: true) }",
            }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({
                "query": "{ a: shape { ... @skip(if: true) { fine } } a: shape { fine } }",
            }),
            json!({ "a": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { inner { fine @skip(if: true) } } shape { inner { fine } } }" }),
            json!({ "shape": { "inner": { "fine": 1 } } }),
        ),
        (
            json!({ "query": "{ either { ... on Other { other } } either { ... on Shape { fine } } }" }),
            json!({ "either": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { broken } shape { fine } }" }),
            json!({ "shape": { "broken": null, "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shapes { broken } shapes { fine } }" }),
            json!({ "shapes": [{ "broken": null, "fine": 1 }, { "broken": null, "fine": 1 }] }),
        ),
        (
            json!({
                "query": "{ ...A ...B } fragment A on Query { shape { broken } } \
                          fragment B on Query { shape { fine } }",
            }),
            json!({ "shape": { "broken": null, "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { inner { broken } } shape { inner { fine } } }" }),
            json!({ "shape": { "inner": { "broken": null, "fine": 1 } } }),
        ),
    ] {
        let answer = post(&app, &body).await;
        assert_eq!(answer["data"], data, "{body}: {answer}");
        assert!(!answer.to_string().contains("__nestrs"), "{answer}");
    }
}

/// The cells of the root's `grid`, each answering `cell`.
fn grid_of(cell: &Value) -> Value {
    json!([[cell, cell], [cell]])
}

/// async-graphql 7.2.1 joins a later copy of one response key into the first only
/// where the first's list items are objects (`insert_value`), so a list of lists
/// selected twice at one place answered one copy's items whole and lost the other's,
/// with no error. The copies merge before validation (`src/field_collection.rs`), a
/// copy below another copy's list included, and each cell answers what both select,
/// its nulls placed, on `POST`, in a batch and over the socket.
#[tokio::test]
async fn a_list_of_lists_selected_twice_otherwise_answers_both_copies() {
    let app = boot().await;
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
            "{ sureShapes { grid { fine } } sureShapes { grid { inner { fine } } } }",
            json!({ "sureShapes": [{ "grid": [[merged.clone()]] }, { "grid": [[merged.clone()]] }] }),
        ),
        (
            "{ either { ... on Shape { grid { fine } } } \
               either { ... on Shape { grid { inner { fine } } } } }",
            json!({ "either": { "grid": [[merged.clone()]] } }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
    let document = "{ g: grid { fine } g: grid { broken } }";
    let answer = post(&app, &json!({ "query": document })).await;
    assert_answers(
        &answer,
        document,
        &json!({ "g": grid_of(&json!({ "fine": 1, "broken": null })) }),
        &[
            json!(["g", 0, 0, "broken"]),
            json!(["g", 0, 1, "broken"]),
            json!(["g", 1, 0, "broken"]),
        ],
    );
    let document = "{ shape { inner { grid { fine } } } shape { inner { grid { broken } } } }";
    let answer = post(&app, &json!({ "query": document })).await;
    assert_answers(
        &answer,
        document,
        &json!({ "shape": { "inner": { "grid": [[{ "fine": 1, "broken": null }]] } } }),
        &[json!(["shape", "inner", "grid", 0, 0, "broken"])],
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
        json!({ "data": { "grid": grid_of(&json!({ "fine": 1 })) } }),
        "{answer}"
    );
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("grid", document);
    let item = socket.next_item("grid").await.expect("the answer");
    assert_eq!(item, json!({ "data": { "grid": grid_of(&merged) } }));
}

/// What no copy loses answers as GraphQL §6.3.2 merges it: copies alike, one a
/// directive leaves out, copies under conditions no one value meets, a list of
/// lists selected once beside another copy of its parent, and a list of objects.
#[tokio::test]
async fn a_list_of_lists_no_copy_loses_answers_its_copies() {
    let app = boot().await;
    let grid = json!([[{ "fine": 1 }, { "fine": 1 }], [{ "fine": 1 }]]);
    for (document, data) in [
        ("{ grid { fine } }", json!({ "grid": grid })),
        ("{ grid { fine } grid { fine } }", json!({ "grid": grid })),
        (
            "{ ...A ...A } fragment A on Query { grid { fine } }",
            json!({ "grid": grid }),
        ),
        (
            "{ ...A ...B } fragment A on Query { grid { fine } } \
             fragment B on Query { grid { fine } }",
            json!({ "grid": grid }),
        ),
        (
            "{ grid { fine } grid @skip(if: true) { inner { fine } } }",
            json!({ "grid": grid }),
        ),
        (
            "{ eithers { ... on Shape { grid { fine } } ... on Other { grid { other } } } }",
            json!({ "eithers": [{ "grid": [[{ "fine": 1 }]] }, { "grid": [] }] }),
        ),
        (
            "{ sureShapes { fine } sureShapes { grid { fine } } }",
            json!({ "sureShapes": [
                { "fine": 1, "grid": [[{ "fine": 1 }]] },
                { "fine": 1, "grid": [[{ "fine": 1 }]] },
            ] }),
        ),
        (
            "{ sureShapes { fine } sureShapes { inner { fine } } }",
            json!({ "sureShapes": [
                { "fine": 1, "inner": { "fine": 1 } },
                { "fine": 1, "inner": { "fine": 1 } },
            ] }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
}

/// What a type condition leaves an object is read: no condition applying to the
/// value, the object answers `{}`, and two selections under distinct conditions,
/// which no one value meets, are no repeat, so one left with nothing is refused
/// nowhere.
#[tokio::test]
async fn an_object_no_type_condition_of_which_applies_answers_an_empty_object() {
    let app = boot().await;
    for (document, data) in [
        (
            "{ either { ... on Other { other } } }",
            json!({ "either": {} }),
        ),
        (
            "{ sureEither { ... on Other { other } } }",
            json!({ "sureEither": {} }),
        ),
        (
            "{ eithers { ... on Other { other } } }",
            json!({ "eithers": [{}, { "other": 2 }] }),
        ),
        (
            "{ either { ...O } } fragment O on Other { other }",
            json!({ "either": {} }),
        ),
        (
            "{ either { ... on Shape { inner { fine @skip(if: true) } } \
               ... on Other { other } } }",
            json!({ "either": { "inner": {} } }),
        ),
        (
            "{ either { ... on Shape { fine } ... on Other { other } } }",
            json!({ "either": { "fine": 1 } }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
}

/// async-graphql 7.2.1 (`Fields::add_set`) applies a condition on a union only where
/// that union is resolved, where GraphQL §6.3.2 applies it to a member wherever one is
/// selected: the condition is rewritten before validation (`src/field_collection.rs`),
/// and answers as the specification reads.
#[tokio::test]
async fn a_condition_on_a_union_applies_wherever_a_member_is_resolved() {
    let app = boot().await;
    for (document, data) in [
        (
            "{ shape { ... on Either { ... on Shape { fine } } } }",
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            "{ shape { ...E } } fragment E on Either { ... on Shape { fine } }",
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            "{ either { ... on Either { ... on Shape { fine } } } }",
            json!({ "either": { "fine": 1 } }),
        ),
        (
            "{ either { ...E } } fragment E on Either { ... on Shape { fine } }",
            json!({ "either": { "fine": 1 } }),
        ),
        (
            "{ shapes { ... on Either { ... on Shape { strict } } } }",
            json!({ "shapes": [null, null] }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(answer["data"], data, "{document}: {answer}");
    }
}

/// The key an object is given leaves no member of a scalar's value, whatever it is
/// called.
#[tokio::test]
async fn a_scalar_beside_an_object_given_its_key_keeps_every_member() {
    let app = boot().await;
    for (document, shape) in [
        ("{ meta shape { fine @skip(if: true) } }", json!({})),
        (
            "{ meta shape { fine } shape { fine } }",
            json!({ "fine": 1 }),
        ),
        (
            "{ shape { inner { fine @skip(if: true) } } meta }",
            json!({ "inner": {} }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(
            answer,
            json!({ "data": { "meta": meta(), "shape": shape } }),
            "{document}"
        );
    }
}

/// The key each object keeps is no key a client wrote: one naming its own the same
/// way reads its own.
#[tokio::test]
async fn a_client_key_spelled_as_the_kept_one_reads_its_own() {
    let answer = post(
        &boot().await,
        &json!({
            "query": "{ shape { __nestrs_key: fine } shape { fine @skip(if: true) } }",
        }),
    )
    .await;
    assert_eq!(
        answer,
        json!({ "data": { "shape": { "__nestrs_key": 1 } } }),
        "{answer}"
    );
}

/// Validation speaks first, as for every refusal the operation's read holds.
#[tokio::test]
async fn a_document_validation_refuses_says_what_validation_says_of_it() {
    let answer = post(
        &boot().await,
        &json!({ "query": "{ shape { fine @skip(if: true) } shape { fine } unknownField }" }),
    )
    .await;
    assert_eq!(
        answer["errors"][0]["message"], r#"Unknown field "unknownField" on type "Query"."#,
        "{answer}"
    );
    assert!(!answer.to_string().contains("__nestrs"), "{answer}");
}

/// Each copy answers its own position.
#[tokio::test]
async fn a_copy_under_another_key_answers_its_own() {
    let app = boot().await;
    for (body, data) in [
        (
            json!({ "query": "{ shape @skip(if: true) { fine } shape { fine } }" }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { fine } shape @include(if: false) { fine } }" }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ a: shape { fine @skip(if: true) } b: shape { fine } }" }),
            json!({ "a": {}, "b": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { inner { fine @skip(if: true) } } a: shape { inner { fine } } }" }),
            json!({ "shape": { "inner": {} }, "a": { "inner": { "fine": 1 } } }),
        ),
        (
            json!({ "query": "{ shape { fine } shape { fine } }" }),
            json!({ "shape": { "fine": 1 } }),
        ),
    ] {
        let answer = post(&app, &body).await;
        assert_eq!(answer, json!({ "data": data }), "{body}");
    }
}

/// Each operation of a batch is read on its own.
#[tokio::test]
async fn each_operation_of_a_batch_keeps_its_objects() {
    let answer = post(
        &boot().await,
        &json!([
            { "query": "{ shape { fine @skip(if: true) } shape { fine } }" },
            { "query": "{ sureShape { fine @skip(if: true) } }" },
        ]),
    )
    .await;
    assert_eq!(
        answer,
        json!([
            { "data": { "shape": { "fine": 1 } } },
            { "data": { "sureShape": {} } },
        ]),
        "{answer}"
    );
}

/// A mutation's root field reads the object it answers as a query's does.
#[tokio::test]
async fn a_mutations_object_left_nothing_answers_an_empty_object() {
    let answer = post(
        &boot().await,
        &json!({ "query": "mutation { reshaped { fine @skip(if: true) } }" }),
    )
    .await;
    assert_eq!(answer, json!({ "data": { "reshaped": {} } }), "{answer}");
}

#[tokio::test]
async fn a_subscription_item_answers_its_nulls() {
    let app = boot().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("shaped", "subscription { shaped { fine broken } }");
    let item = socket.next_item("shaped").await.expect("the item");
    assert_answers(
        &item,
        "subscription",
        &json!({ "shaped": { "fine": 1, "broken": null } }),
        &[json!(["shaped", "broken"])],
    );
    socket.subscribe("left", "subscription { shaped { fine @skip(if: true) } }");
    let item = socket.next_item("left").await.expect("the item");
    assert_eq!(item, json!({ "data": { "shaped": {} } }), "{item}");
    socket.subscribe(
        "copies",
        "subscription { shaped { inner { fine @skip(if: true) } inner { fine } } }",
    );
    let item = socket.next_item("copies").await.expect("the item");
    assert_eq!(
        item,
        json!({ "data": { "shaped": { "inner": { "fine": 1 } } } }),
        "{item}"
    );
    socket.subscribe("query", "{ shape { fine @skip(if: true) } shape { fine } }");
    let item = socket.next_item("query").await.expect("the item");
    assert_eq!(
        item,
        json!({ "data": { "shape": { "fine": 1 } } }),
        "{item}"
    );
}
