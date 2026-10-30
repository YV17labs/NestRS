//! Covers `src/created.rs`: a `Created` answer is a `201` whose `Location`
//! names the collection the caller addressed, and its route documents both
//! off the return type.

use nest_rs_core::{Discovery, module};
use nest_rs_http::{
    ApiVersioning, Created, DEFAULT_VERSION_HEADER, HttpConfig, HttpControllerMeta, Json, Result,
    StatusCode, controller, header, routes,
};
use nest_rs_testing::TestApp;

const ID: &str = "018f3f9c-0000-7000-8000-000000000001";

/// What an alias of the answer still documents.
type Minted = Created<Json<String>>;

#[controller(path = "/orgs", version = "1")]
struct OrgsController;

#[routes]
impl OrgsController {
    #[post("/")]
    #[public]
    async fn create(&self) -> Result<Created<Json<String>>> {
        Ok(Created::at(ID, Json("acme".to_owned())))
    }

    #[post("/aliased")]
    #[public]
    async fn aliased(&self) -> Result<Minted> {
        Ok(Created::at(ID, Json("acme".to_owned())))
    }
}

#[module(providers = [OrgsController])]
struct OrgsModule;

#[tokio::test]
async fn the_location_names_the_path_a_header_selected_version_left_unversioned() {
    let client = crate::boot_on::<OrgsModule>(HttpConfig {
        versioning: ApiVersioning::Header,
        global_prefix: Some("/api".into()),
        ..HttpConfig::default()
    })
    .await;

    let created = client
        .post("/api/orgs")
        .header(DEFAULT_VERSION_HEADER, "1")
        .send()
        .await;

    created.assert_status(StatusCode::CREATED);
    created.assert_header(header::LOCATION, format!("/api/orgs/{ID}"));
    created.assert_json("acme").await;
}

#[tokio::test]
async fn the_document_reads_the_status_the_location_and_the_body_off_the_return_type() {
    let app = TestApp::for_module::<OrgsModule>().await.expect("boots");
    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let routes = controllers[0].meta.routes();

    for handler in ["create", "aliased"] {
        let route = routes
            .iter()
            .find(|route| route.handler() == handler)
            .expect("the route is declared");
        assert_eq!(route.success_status(), 201, "{handler}");
        assert!(route.sets_location(), "{handler}");
        assert!(!route.sets_next_link(), "{handler}");
    }
    let spelled = routes
        .iter()
        .find(|route| route.handler() == "create")
        .expect("the route is declared");
    assert!(
        spelled.response().is_some(),
        "the body spelled inside `Created` is documented",
    );
}
