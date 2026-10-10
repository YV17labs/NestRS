//! A `Piped<P, E>` refusal is the edge's `400` problem, and its `detail` says
//! what the value had to be, never the value.

use nest_rs_core::module;
use nest_rs_http::{Piped, controller, routes};
use nest_rs_pipes::ParseArray;
use poem::http::StatusCode;
use poem::web::Path;

#[controller(path = "/pipe")]
struct PipeController;

#[routes]
impl PipeController {
    #[get("/ids/{ids}")]
    async fn ids(&self, ids: Piped<ParseArray<u64>, Path<String>>) -> String {
        format!("{:?}", ids.into_inner())
    }
}

#[module(providers = [PipeController])]
struct PipeModule;

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
            .is_some_and(|detail| detail.contains("u64")),
        "the refusal names what an item must be: {problem}",
    );
}
