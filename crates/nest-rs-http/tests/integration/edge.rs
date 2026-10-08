//! Path normalization at the transport edge (`src/edge.rs`), through the whole
//! composed stack rather than the layer alone.

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use poem::http::StatusCode;

#[controller(path = "/kitchen")]
struct KitchenController;

#[routes]
impl KitchenController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "kitchen"
    }

    #[get("/items/:id")]
    async fn item(&self, id: poem::web::Path<String>) -> String {
        format!("item {}", id.0)
    }

    #[get("/search")]
    async fn search(&self, req: &poem::Request) -> String {
        req.uri().query().unwrap_or("none").to_owned()
    }
}

#[module(providers = [KitchenController])]
struct KitchenModule;

#[tokio::test]
async fn a_collection_answers_with_and_without_the_trailing_slash() {
    let client = crate::boot::<KitchenModule>().await;

    let bare = client.get("/kitchen").send().await;
    bare.assert_status_is_ok();
    bare.assert_text("kitchen").await;

    let slashed = client.get("/kitchen/").send().await;
    slashed.assert_status_is_ok();
    slashed.assert_text("kitchen").await;
}

#[tokio::test]
async fn a_captured_segment_is_the_same_with_the_slash() {
    let client = crate::boot::<KitchenModule>().await;

    let slashed = client.get("/kitchen/items/42/").send().await;
    slashed.assert_status_is_ok();
    slashed.assert_text("item 42").await;
}

#[tokio::test]
async fn an_unmounted_path_still_answers_404() {
    let client = crate::boot::<KitchenModule>().await;

    let missing = client.get("/pantry/").send().await;
    missing.assert_status(StatusCode::NOT_FOUND);
    missing.assert_content_type("application/problem+json");
}

#[tokio::test]
async fn the_query_string_survives_normalization() {
    let client = crate::boot::<KitchenModule>().await;

    let resp = client.get("/kitchen/search/?q=1&sort=asc").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("q=1&sort=asc").await;
}
