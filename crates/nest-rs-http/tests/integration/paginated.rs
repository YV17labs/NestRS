//! Covers `src/paginated.rs`: a `Paginated` answer links the next page on the
//! path the caller addressed, the last page links none, and its route
//! documents the `Link` off the return type.

use nest_rs_core::{Discovery, module};
use nest_rs_http::{
    HttpConfig, HttpControllerMeta, Json, Paginated, Query, Result, controller, header, routes,
};
use nest_rs_testing::TestApp;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct After {
    after: Option<u32>,
}

#[controller(path = "/tracks")]
struct TracksController;

#[routes]
impl TracksController {
    #[get("/")]
    #[public]
    async fn list(&self, page: Query<After>) -> Result<Paginated<Json<Vec<u32>>>> {
        Ok(match page.0.after {
            None => Paginated::new(Json(vec![1, 2]), 2, Some(2)),
            Some(_) => Paginated::new(Json(vec![3]), 2, None::<u32>),
        })
    }
}

#[module(providers = [TracksController])]
struct TracksModule;

#[tokio::test]
async fn a_page_another_follows_links_it_under_the_global_prefix_and_the_last_links_none() {
    let client = crate::boot_on::<TracksModule>(HttpConfig {
        global_prefix: Some("/api".into()),
        ..HttpConfig::default()
    })
    .await;

    let first = client.get("/api/tracks").send().await;
    first.assert_status_is_ok();
    first.assert_header(header::LINK, "</api/tracks?first=2&after=2>; rel=\"next\"");
    first.assert_json([1, 2]).await;

    let last = client.get("/api/tracks?after=2").send().await;
    last.assert_status_is_ok();
    last.assert_header_is_not_exist(header::LINK);
}

#[tokio::test]
async fn the_document_reads_the_link_and_the_body_off_the_return_type() {
    let app = TestApp::for_module::<TracksModule>().await.expect("boots");
    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let route = &controllers[0].meta.routes()[0];

    assert_eq!(route.success_status(), 200);
    assert!(route.sets_next_link());
    assert!(!route.sets_location());
    assert!(
        route.response().is_some(),
        "the body spelled inside `Paginated`"
    );
}
