//! `src/path.rs`: `Path<T>` through a mounted route — the router's decoding,
//! then the parameters read by position or by name.

use nest_rs_http::{FromRequest, IntoResponse, Method, Path};
use nest_rs_testing::LogCapture;
use poem::http::StatusCode;
use serde::Deserialize;

#[derive(Deserialize)]
struct Member {
    id: u64,
    org: String,
}

async fn members() -> crate::Client {
    crate::vocabulary_route(
        Method::GET,
        "/orgs/{org}/members/{id}",
        |req, mut body| async move {
            let Path((org, id)) = Path::<(String, u64)>::from_request(&req, &mut body).await?;
            let Path(member) = Path::<Member>::from_request(&req, &mut body).await?;
            Ok(format!("{org}#{id} {}#{}", member.org, member.id).into_response())
        },
    )
    .await
}

#[tokio::test]
async fn path_binds_a_tuple_by_position_and_a_struct_by_name() {
    let client = members().await;
    let (status, _, body) =
        crate::answered(client.get("/orgs/a%2Fb+c/members/7").send().await).await;
    assert_eq!(status, 200);
    assert_eq!(
        body, "a/b+c#7 a/b+c#7",
        "a parameter is percent-decoded once, a `+` stays itself in a path"
    );
}

#[tokio::test]
async fn a_path_decode_failure_names_the_parameter_not_its_value() {
    let client = members().await;
    let resp = client.get("/orgs/acme/members/sk_live_51").send().await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_content_type("application/problem+json");
    let body = resp.0.into_body().into_string().await.expect("a body");
    let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem");
    assert_eq!(problem["detail"], "path parameter `id` is not an integer");
    assert!(!body.contains("sk_live"), "{body}");
}

/// What the operator reads of a handler whose `Path` its route cannot bind.
const UNBOUND: &str = "a handler reads path parameters its route cannot bind";

#[tokio::test]
async fn a_handler_reading_a_parameter_its_route_does_not_declare_answers_an_opaque_500() {
    #[derive(Deserialize)]
    struct Elsewhere {
        #[expect(dead_code, reason = "read only to be refused")]
        slug: String,
    }
    let logs = LogCapture::install();
    let client = crate::vocabulary_route(Method::GET, "/posts/{id}", |req, mut body| async move {
        Path::<Elsewhere>::from_request(&req, &mut body).await?;
        Ok("unreachable".into_response())
    })
    .await;
    let resp = client.get("/posts/sk_live_51").send().await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(
        !body.contains("slug"),
        "the signature mistake stays in the log: {body}"
    );
    assert!(!body.contains("sk_live"), "{body}");

    let filed = logs.expect_one(nest_rs_http::target::HTTP, UNBOUND);
    assert_eq!(filed.level, "error");
    assert_eq!(
        filed.field("error").as_deref(),
        Some("the handler reads `slug`, a path parameter its route does not declare")
    );
}

#[tokio::test]
async fn a_malformed_parameter_is_the_client_s_and_files_no_error() {
    let logs = LogCapture::install();
    let client = members().await;
    client
        .get("/orgs/acme/members/sk_live_51")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    logs.expect_none(nest_rs_http::target::HTTP, UNBOUND);
}
