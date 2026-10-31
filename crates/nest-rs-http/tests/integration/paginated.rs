//! Covers `src/paginated.rs`: a `Paginated` answer links the next page on the
//! path the caller addressed, whoever returns it, the last page links none,
//! and its route documents the `Link` and the body off the return type.

use nest_rs_core::{ContainerBuilder, Discoverable, Discovery, module};
use nest_rs_http::{
    HttpConfig, HttpControllerMeta, HttpEndpointMeta, IntoResponse, Json, Method, Paginated, Query,
    Result, controller, endpoint_fn, header, routes,
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

/// A self-mount paging what it lists, as a package's audit feed does.
struct FeedMount;

impl Discoverable for FeedMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<FeedMount, HttpEndpointMeta>(
            HttpEndpointMeta::new(&crate::PUBLIC_MOUNT, "/feed", "feed", |_, routes| {
                routes.route(
                    Method::GET,
                    "/feed",
                    endpoint_fn(|_| async {
                        Ok(Paginated::new(Json(vec![1, 2]), 2, Some(2)).into_response())
                    }),
                );
            })
            .owned_by("FeedMount"),
        )
    }
}

#[module(providers = [FeedMount])]
struct FeedModule;

#[tokio::test]
async fn a_self_mounts_page_links_the_next_on_the_path_the_caller_addressed() {
    let client = crate::boot_on::<FeedModule>(HttpConfig {
        global_prefix: Some("/api".into()),
        ..HttpConfig::default()
    })
    .await;

    let first = client.get("/api/feed").send().await;

    first.assert_status_is_ok();
    first.assert_header(header::LINK, "</api/feed?first=2&after=2>; rel=\"next\"");
}
