//! The transport edge (`src/edge.rs`) through the whole composed stack rather
//! than the layer alone: path normalization, and a handler that unwinds.

use std::panic::AssertUnwindSafe;

use futures_util::FutureExt as _;
use nest_rs_core::module;
use nest_rs_core::panic::{FIELD, LOCATION_FIELD};
use nest_rs_http::{controller, routes};
use nest_rs_testing::LogCapture;
use poem::http::StatusCode;

#[controller(path = "/kitchen")]
struct KitchenController;

#[routes]
impl KitchenController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "kitchen"
    }

    #[get("/items/{id}")]
    async fn item(&self, id: poem::web::Path<String>) -> String {
        format!("item {}", id.0)
    }

    #[get("/search")]
    async fn search(&self, req: &poem::Request) -> String {
        req.uri().query().unwrap_or("none").to_owned()
    }

    #[get("/shelves/{n}")]
    async fn shelf(&self, n: poem::web::Path<u32>) -> String {
        format!("shelf {}", n.0)
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

/// An `Err` escaping the route tree is rendered inside the header stamp, so the
/// router's own `404` and an extractor's `400` carry the security headers too.
#[tokio::test]
async fn an_error_the_route_tree_answers_carries_the_edge_s_headers() {
    let client = crate::boot::<KitchenModule>().await;

    let missing = client.get("/pantry").send().await;
    missing.assert_status(StatusCode::NOT_FOUND);
    missing.assert_content_type("application/problem+json");
    missing.assert_header("x-content-type-options", "nosniff");

    let unparsed = client.get("/kitchen/shelves/top").send().await;
    unparsed.assert_status(StatusCode::BAD_REQUEST);
    unparsed.assert_content_type("application/problem+json");
    unparsed.assert_header("x-content-type-options", "nosniff");
}

#[controller(path = "/oven")]
struct OvenController;

#[routes]
impl OvenController {
    #[get("/")]
    async fn bake(&self) -> &'static str {
        panic!("the oven caught fire")
    }
}

#[module(providers = [OvenController])]
struct OvenModule;

#[nest_rs_core::main]
async fn ask_the_oven() -> bool {
    let client = crate::boot::<OvenModule>().await;
    AssertUnwindSafe(client.get("/oven").send())
        .catch_unwind()
        .await
        .is_err()
}

/// No unit contains a handler's unwind, so the process hook files it beside the
/// request's `outcome = panic` line.
#[test]
fn a_handler_that_panics_is_filed_by_the_process_hook_beside_its_line() {
    let logs = LogCapture::install();

    let unwound = ask_the_oven();
    // Rust's own hook back, so an assertion failing below prints.
    drop(std::panic::take_hook());

    assert!(unwound, "the panic reached the caller");

    let hook = logs.expect_one(
        nest_rs_core::target::APP,
        "panicked where no unit of work contains it",
    );
    assert_eq!(hook.level, "error");
    assert_eq!(hook.field(FIELD).as_deref(), Some("the oven caught fire"));
    let location = hook.field(LOCATION_FIELD).unwrap_or_default();
    assert!(
        location.contains("tests/integration/edge.rs:"),
        "where it panicked: {hook:#?}"
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{line:#?}"
    );
}
