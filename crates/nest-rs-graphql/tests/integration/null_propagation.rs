//! Covers `src/null_propagation.rs`: a field that errs answers `null` beside
//! its error, and a Non-Null one nulls its nearest nullable parent — `data`
//! itself at the root (GraphQL, October 2021, §6.4.4) — on `POST`, in a batch
//! and over the socket.

use nest_rs_core::module;
use nest_rs_graphql::async_graphql::futures_util::stream as futures_stream;
use nest_rs_graphql::async_graphql::{Object, Result};
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

    #[subscription]
    #[public]
    async fn shaped(&self) -> impl futures_stream::Stream<Item = Shape> {
        futures_stream::iter([Shape])
    }
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
/// that ran, its nulls placed, on `POST` and in a batch alike.
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
        // Both copies run: the `null` may hide what the other answered, so it stands.
        (
            json!({
                "query": "query($x: Boolean!) { shape { broken } shape @include(if: $x) { fine } }",
                "variables": { "x": true },
            }),
            json!({ "shape": null }),
        ),
    ] {
        let answer = post(&app, &body).await;
        assert_eq!(answer["data"], data, "{body}: {answer}");
    }
}

/// async-graphql (`7.2.1`, `do_resolve_container`) answers `null` for an object
/// with no key to answer, and an object every selection of which a directive
/// leaves out has none. At a nullable position that `null` reads as a resolver's
/// `None` does, so it stands. When this fails, a release answers `{}`.
#[tokio::test]
async fn a_nullable_object_every_selection_of_which_is_left_out_answers_null() {
    let app = boot().await;
    for (document, data) in [
        (
            "{ shape { fine @skip(if: true) } }",
            json!({ "shape": null }),
        ),
        (
            "{ shapes { fine @skip(if: true) } }",
            json!({ "shapes": [null, null] }),
        ),
        (
            "{ shape { fine @skip(if: true) } shape { broken @include(if: false) } }",
            json!({ "shape": null }),
        ),
    ] {
        let answer = post(&app, &json!({ "query": document })).await;
        assert_eq!(answer, json!({ "data": data }), "{document}");
    }
}

/// A Non-Null position has no `None`: a `null` with no error there can only be
/// an object every selection of which a directive left out, and it answers as
/// GraphQL §6.3 does.
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

/// What the operation answers once a directive leaves the selection under
/// `key` nothing to answer beside another that answers.
fn hidden(key: &str) -> String {
    format!(
        r#"the operation selects "{key}" more than once at one place, and its directives leave one of those selections nothing to answer: put the directive on that selection, to leave it out whole"#
    )
}

/// async-graphql answers `null` for a selection left nothing to answer, and
/// drops what the other selection answers once that `null` finished first, with
/// no error: `null_propagation::tests::` pins it, so no such document runs.
#[tokio::test]
async fn a_selection_left_nothing_to_answer_beside_another_is_refused() {
    let app = boot().await;
    for (body, key) in [
        (
            json!({ "query": "{ shape { fine @skip(if: true) } shape { fine } }" }),
            "shape",
        ),
        (
            json!({ "query": "{ shape { fine } shape { fine @skip(if: true) } }" }),
            "shape",
        ),
        (
            json!({
                "query": "query($x: Boolean = false) { \
                          shape { fine @include(if: $x) } shape { fine } }",
            }),
            "shape",
        ),
        (
            json!({
                "query": "query($x: Boolean!) { shape { fine @include(if: $x) } shape { fine } }",
                "variables": { "x": false },
            }),
            "shape",
        ),
        (
            json!({ "query": "{ ... { shape { fine @skip(if: true) } } shape { fine } }" }),
            "shape",
        ),
        (
            json!({ "query": "{ shapes { fine @skip(if: true) } shapes { fine } }" }),
            "shapes",
        ),
        (
            json!({ "query": "{ sureShape { fine @skip(if: true) } sureShape { fine } }" }),
            "sureShape",
        ),
        (
            json!({
                "query": "{ ...A ...B } fragment A on Query { shape { ...C } } \
                          fragment B on Query { shape { fine } } \
                          fragment C on Shape { fine @skip(if: true) }",
            }),
            "shape",
        ),
        (
            json!({
                "query": "{ a: shape { ... @skip(if: true) { fine } } a: shape { fine } }",
            }),
            "a",
        ),
        (
            json!({ "query": "{ shape { inner { fine @skip(if: true) } } shape { inner { fine } } }" }),
            "inner",
        ),
        (
            json!({ "query": "{ shape { fine @skip(if: true) } shape { broken } }" }),
            "shape",
        ),
    ] {
        let answer = post(&app, &body).await;
        assert_eq!(answer["data"], Value::Null, "{body}: {answer}");
        assert_eq!(
            answer["errors"][0]["message"],
            hidden(key),
            "{body}: {answer}"
        );
        assert_eq!(
            answer["errors"].as_array().map(Vec::len),
            Some(1),
            "{answer}"
        );
        assert_eq!(
            answer["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
            "{answer}"
        );
    }
}

#[tokio::test]
async fn a_refused_selection_points_at_the_one_left_nothing_to_answer() {
    let answer = post(
        &boot().await,
        &json!({ "query": "{\n  shape { fine }\n  shape { fine @skip(if: true) }\n}" }),
    )
    .await;
    assert_eq!(
        answer["errors"][0]["locations"],
        json!([{ "line": 3, "column": 9 }]),
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
}

/// None of these hides an answer, so each runs.
#[tokio::test]
async fn a_selection_hiding_no_answer_is_not_refused() {
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
            json!({
                "query": "query($x: Boolean = true) { \
                          shape { fine @include(if: $x) } shape { fine } }",
            }),
            json!({ "shape": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ a: shape { fine @skip(if: true) } b: shape { fine } }" }),
            json!({ "a": null, "b": { "fine": 1 } }),
        ),
        (
            json!({ "query": "{ shape { inner { fine @skip(if: true) } } a: shape { inner { fine } } }" }),
            json!({ "shape": { "inner": null }, "a": { "inner": { "fine": 1 } } }),
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

/// In a batch only the operation holding it is refused.
#[tokio::test]
async fn a_batch_refuses_only_its_operation_hiding_an_answer() {
    let answer = post(
        &boot().await,
        &json!([
            { "query": "{ shape { fine @skip(if: true) } shape { fine } }" },
            { "query": "{ sureShape { fine @skip(if: true) } }" },
        ]),
    )
    .await;
    assert_eq!(
        answer[0]["errors"][0]["message"],
        hidden("shape"),
        "{answer}"
    );
    assert_eq!(
        answer[1],
        json!({ "data": { "sureShape": {} } }),
        "{answer}"
    );
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
        "hiding",
        "subscription { shaped { inner { fine @skip(if: true) } inner { fine } } }",
    );
    let item = socket.next_item("hiding").await.expect("the refusal");
    assert_eq!(item["errors"][0]["message"], hidden("inner"), "{item}");
}
