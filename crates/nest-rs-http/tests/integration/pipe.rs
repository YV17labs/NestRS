//! A `Piped<P, E>` refusal is the edge's `400` problem, and its `detail` says
//! what the value had to be, never the value: a `400` is logged by proxies and
//! kept by caches, and a client's input is not the reply's to repeat.

use nest_rs_core::module;
use nest_rs_http::{Piped, controller, routes};
use nest_rs_pipes::ParseArray;
use poem::http::StatusCode;
use poem::web::Path;

#[controller(path = "/pipe")]
struct PipeController;

#[routes]
impl PipeController {
    #[get("/ids/:ids")]
    async fn ids(&self, ids: Piped<ParseArray<u64>, Path<String>>) -> String {
        format!("{:?}", ids.into_inner())
    }
}

#[module(providers = [PipeController])]
struct PipeModule;

/// A list item the pipe refuses is said without its value. `ParseArray`'s
/// refusal quoted the item, and the edge answers a pipe's refusal as the
/// problem's `detail`.
#[tokio::test]
async fn a_refused_list_item_is_never_quoted_in_the_problem() {
    let client = crate::boot::<PipeModule>().await;

    let resp = client
        .get("/pipe/ids/1,sk_live_51HsecretTOKEN,3")
        .send()
        .await;

    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_content_type("application/problem+json");
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a readable body");
    assert!(
        !body.contains("sk_live"),
        "the problem quotes the item: {body}"
    );
    let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem document");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "the refusal still says what was wrong: {problem}",
    );
}
