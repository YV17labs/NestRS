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

    #[subscription]
    #[public]
    async fn shaped(&self) -> impl futures_stream::Stream<Item = Shape> {
        futures_stream::iter([Shape])
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [ShapeResolver])]
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
}
