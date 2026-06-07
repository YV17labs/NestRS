//! URI versioning (`#[controller(version = "1")]`) and exception filters
//! (`#[use_filters]`), end-to-end through the HTTP harness.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_http::{Filter, RequestSnapshot, async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Error, Response};

#[injectable]
#[derive(Default)]
struct TeapotFilter;

impl Layer for TeapotFilter {}

#[async_trait]
impl Filter for TeapotFilter {
    async fn filter(&self, _req: &RequestSnapshot, _error: Error) -> Response {
        Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .body("filtered")
    }
}

#[controller(path = "/widgets", version = "1")]
struct WidgetController;

#[routes]
impl WidgetController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "widgets"
    }

    #[get("/boom")]
    #[use_filters(TeapotFilter)]
    async fn boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }

    #[get("/raw-boom")]
    async fn raw_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[controller(path = "/gadgets")]
#[use_filters(TeapotFilter)]
struct GadgetController;

#[routes]
impl GadgetController {
    #[get("/boom")]
    async fn gadget_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[module(providers = [TeapotFilter, WidgetController, GadgetController])]
struct WidgetModule;

#[tokio::test]
async fn versioned_controller_is_served_under_v_prefix() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/v1/widgets").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("widgets").await;
}

#[tokio::test]
async fn unversioned_path_is_not_mounted() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/widgets").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn per_route_filter_maps_the_error() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/v1/widgets/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    resp.assert_text("filtered").await;
}

#[tokio::test]
async fn route_without_filter_uses_default_error() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/v1/widgets/raw-boom").send().await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn controller_level_filter_maps_errors_without_a_per_route_filter() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/gadgets/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    resp.assert_text("filtered").await;
}
