//! `src/json.rs`: `Json<T>` read and answered through a mounted route.

use nest_rs_http::StatusCode;
use nest_rs_http::{FromRequest, HttpConfig, IntoResponse, Json, Method};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Order {
    quantity: u32,
}

async fn orders_on(config: HttpConfig) -> crate::Client {
    crate::vocabulary_route_on(
        config,
        Method::POST,
        "/orders",
        |req, mut body| async move {
            let Json(order) = Json::<Order>::from_request(&req, &mut body).await?;
            Ok(Json(Order {
                quantity: order.quantity * 2,
            })
            .into_response())
        },
    )
    .await
}

#[tokio::test]
async fn a_json_body_reads_and_answers_as_json() {
    let client = orders_on(HttpConfig::default()).await;
    let resp = client
        .post("/orders")
        .body_json(&serde_json::json!({ "quantity": 2 }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_content_type("application/json; charset=utf-8");
    resp.assert_json(serde_json::json!({ "quantity": 4 })).await;
}

#[tokio::test]
async fn each_refusal_answers_a_problem_that_never_quotes_the_request() {
    let client = orders_on(HttpConfig {
        max_body_bytes: Some(32),
        ..HttpConfig::default()
    })
    .await;
    let cases = [
        (
            "text/plain; sk_live_51",
            r#"{"quantity":2}"#.to_owned(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "application/json",
            r#"{"quantity":"sk_live_51"}"#.to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "application/json",
            format!(r#"{{"quantity":2,"pad":"{}"}}"#, "x".repeat(64)),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ];
    for (content_type, body, status) in cases {
        let resp = client
            .post("/orders")
            .content_type(content_type)
            .body(body)
            .send()
            .await;
        resp.assert_status(status);
        resp.assert_content_type("application/problem+json");
        let said = resp.0.into_body().into_string().await.expect("a body");
        assert!(!said.contains("sk_live"), "{status}: {said}");
    }
}
