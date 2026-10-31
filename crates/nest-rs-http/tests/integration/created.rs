//! Covers `src/created.rs`: a `Created` answer is a `201` whose `Location`
//! names the collection the caller addressed, whoever returns it — a route,
//! a self-mount's entry, a route registered by hand — and its route documents
//! the status, the `Location` and the body off the return type.

use std::borrow::Cow;

use nest_rs_core::{ContainerBuilder, Discoverable, Discovery, module};
use nest_rs_http::Result as Answer;
use nest_rs_http::{
    ApiVersioning, Created, DEFAULT_VERSION_HEADER, HttpConfig, HttpControllerMeta,
    HttpEndpointMeta, IntoResponse, Json, Method, Result, StatusCode, controller, endpoint_fn,
    header, routes,
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

    #[post("/renamed")]
    #[public]
    async fn renamed(&self) -> Answer<Created<Json<String>>> {
        Ok(Created::at(ID, Json("acme".to_owned())))
    }

    #[post("/borrowed")]
    #[public]
    async fn borrowed(&self) -> Result<Created<Json<Cow<'_, str>>>> {
        Ok(Created::at(ID, Json(Cow::Borrowed("acme"))))
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

    for handler in ["create", "aliased", "renamed", "borrowed"] {
        let route = routes
            .iter()
            .find(|route| route.handler() == handler)
            .expect("the route is declared");
        assert_eq!(route.success_status(), 201, "{handler}");
        assert!(route.sets_location(), "{handler}");
        assert!(!route.sets_next_link(), "{handler}");
        let body = route
            .response()
            .map(|schema_of| schema_of(&mut schemars::SchemaGenerator::default()));
        assert_eq!(
            body.as_ref().and_then(|schema| schema.get("type")),
            Some(&serde_json::json!("string")),
            "{handler}: the body inside `Created` is documented by its type",
        );
    }
}

/// A self-mount minting what it is posted, as a package's webhook surface does.
struct HooksMount;

impl Discoverable for HooksMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<HooksMount, HttpEndpointMeta>(
            HttpEndpointMeta::new(&crate::PUBLIC_MOUNT, "/hooks", "hooks", |_, routes| {
                routes.route(
                    Method::POST,
                    "/hooks",
                    endpoint_fn(|_| async { Ok(Created::at(ID, Json("hook")).into_response()) }),
                );
            })
            .owned_by("HooksMount"),
        )
    }
}

#[module(providers = [HooksMount])]
struct HooksModule;

/// A controller minting on a route it registers by hand.
struct HandMinted;

impl Discoverable for HandMinted {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let meta = nest_rs_http::__private::http_controller_meta(
            "HandMinted",
            "hand_minted",
            "/minted",
            &[],
            Vec::new(),
            |_, routes| {
                routes.route(
                    Method::POST,
                    "/minted",
                    endpoint_fn(|_| async { Ok(Created::at(ID, Json("acme")).into_response()) }),
                );
            },
        );
        builder
            .provide(HandMinted)
            .attach_meta::<HandMinted, HttpControllerMeta>(meta)
    }
}

#[module(providers = [HandMinted])]
struct HandMintedModule;

fn prefixed() -> HttpConfig {
    HttpConfig {
        global_prefix: Some("/api".into()),
        ..HttpConfig::default()
    }
}

#[tokio::test]
async fn a_self_mounts_created_answer_names_the_path_the_caller_addressed() {
    let client = crate::boot_on::<HooksModule>(prefixed()).await;

    let created = client.post("/api/hooks").send().await;

    created.assert_status(StatusCode::CREATED);
    created.assert_header(header::LOCATION, format!("/api/hooks/{ID}"));
    created.assert_json("hook").await;
}

#[tokio::test]
async fn a_created_answer_on_a_route_registered_by_hand_names_the_path_the_caller_addressed() {
    let client = crate::boot_on::<HandMintedModule>(prefixed()).await;

    let created = client.post("/api/minted").send().await;

    created.assert_status(StatusCode::CREATED);
    created.assert_header(header::LOCATION, format!("/api/minted/{ID}"));
}
