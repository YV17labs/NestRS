//! Covers `src/document.rs` — what the served document says about a version.

use nest_rs_core::module;
use nest_rs_http::poem::http::HeaderName;
use nest_rs_http::{
    ApiVersioning, DEFAULT_VERSION_HEADER, HttpConfig, HttpModule, HttpTransport, VersionSelector,
    controller, routes,
};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule, OpenApiSetup};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use serde_json::Value;

#[controller(path = "/posts", version = "1")]
struct PostsV1Controller;

#[routes]
impl PostsV1Controller {
    #[get("/")]
    async fn list(&self) -> String {
        "v1".into()
    }
}

#[controller(path = "/posts", version = "2")]
struct PostsV2Controller;

#[routes]
impl PostsV2Controller {
    #[get("/")]
    async fn list(&self) -> String {
        "v2".into()
    }

    #[get("/drafts")]
    async fn drafts(&self) -> String {
        "[]".into()
    }
}

#[controller(path = "/reports", version = ["1", "2"])]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "reports".into()
    }
}

#[controller(path = "/probe")]
struct ProbeController;

#[routes]
impl ProbeController {
    #[get("/type")]
    async fn r#type(&self) -> String {
        "raw".into()
    }
}

#[controller(path = "/health")]
struct HealthController;

#[routes]
impl HealthController {
    #[get("/")]
    async fn live(&self) -> String {
        "ok".into()
    }
}

// Pinned: the unpinned `enabled` default depends on the environment profile.
fn openapi() -> OpenApiSetup {
    OpenApiModule::for_root(OpenApiConfig {
        enabled: true,
        emit_document: false,
        ..OpenApiConfig::default()
    })
}

fn http(versioning: ApiVersioning, default_version: Option<&str>) -> HttpConfig {
    HttpConfig {
        versioning,
        default_version: default_version.map(str::to_owned),
        ..HttpConfig::default()
    }
}

#[module(
    imports = [openapi(), HttpModule::for_root(http(ApiVersioning::Header, None))],
    providers = [PostsV1Controller, PostsV2Controller, HealthController],
)]
struct HeaderVersionedApp;

#[module(
    imports = [openapi(), HttpModule::for_root(http(ApiVersioning::Header, Some("1")))],
    providers = [PostsV1Controller, PostsV2Controller, HealthController],
)]
struct DefaultedApp;

#[module(
    imports = [openapi(), HttpModule::for_root(http(ApiVersioning::Uri, None))],
    providers = [PostsV1Controller, PostsV2Controller, HealthController],
)]
struct UriVersionedApp;

#[module(
    imports = [openapi(), HttpModule::for_root(http(ApiVersioning::Uri, None))],
    providers = [ReportsController, HealthController],
)]
struct MultiVersionApp;

#[module(imports = [openapi()], providers = [ProbeController])]
struct RawIdentApp;

// `TestApp`'s default transport resolves no version per request.
fn versioned_transport(default_version: Option<&str>) -> HttpTransport {
    HttpTransport::new().api_versioning(VersionSelector::new(
        ApiVersioning::Header,
        HeaderName::from_static(DEFAULT_VERSION_HEADER),
        default_version.map(str::to_owned),
    ))
}

async fn document(app: &TestApp, path: &str) -> Value {
    let resp = app.http().get(path).send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_bytes().await.expect("a body");
    serde_json::from_slice(&body).expect("the document is JSON")
}

fn operation_id<'a>(document: &'a Value, path: &str, method: &str) -> Option<&'a str> {
    document["paths"][path][method]["operationId"].as_str()
}

fn version_parameter<'a>(operation: &'a Value, header: &str) -> Option<&'a Value> {
    operation["parameters"]
        .as_array()?
        .iter()
        .find(|p| p["in"] == "header" && p["name"] == header)
}

#[tokio::test]
async fn a_header_versioned_document_names_the_paths_a_client_calls() {
    let app = TestApp::builder()
        .module::<HeaderVersionedApp>()
        .http(versioned_transport(None))
        .build()
        .await
        .expect("boots");
    let doc = document(&app, "/api-json").await;
    let paths = doc["paths"].as_object().expect("paths");

    assert!(
        paths.contains_key("/posts") && paths.contains_key("/health"),
        "the client-facing paths are the documented ones: {:?}",
        paths.keys().collect::<Vec<_>>(),
    );
    assert!(
        !paths.keys().any(|path| path.starts_with("/v")),
        "and the mounted `/v{{n}}` prefix — a 404 for every client — is not: {:?}",
        paths.keys().collect::<Vec<_>>(),
    );

    let parameter = version_parameter(&doc["paths"]["/posts"]["get"], DEFAULT_VERSION_HEADER)
        .expect("a versioned operation carries the version parameter");
    assert_eq!(
        parameter["required"], true,
        "with no default version, stating one is not optional: {parameter}",
    );
    assert!(
        parameter["schema"]["enum"].is_array(),
        "the schema enumerates the versions that serve this path: {parameter}",
    );
    assert!(
        version_parameter(&doc["paths"]["/health"]["get"], DEFAULT_VERSION_HEADER).is_none(),
        "an unversioned operation asks for no version: {doc}",
    );
}

#[tokio::test]
async fn each_version_gets_a_document_that_describes_only_it() {
    let app = TestApp::builder()
        .module::<HeaderVersionedApp>()
        .http(versioned_transport(None))
        .build()
        .await
        .expect("boots");

    let v1 = document(&app, "/api-json/v1").await;
    assert!(
        v1["paths"]["/posts/drafts"].is_null(),
        "v2's own route is absent from the document that claims v1: {v1}",
    );
    assert_eq!(
        version_parameter(&v1["paths"]["/posts"]["get"], DEFAULT_VERSION_HEADER)
            .expect("a version parameter")["schema"]["enum"],
        serde_json::json!(["1"]),
        "and the operation it does describe names the version it is: {v1}",
    );

    let v2 = document(&app, "/api-json/v2").await;
    assert!(
        v2["paths"]["/posts/drafts"]["get"].is_object(),
        "v2's document describes v2's routes: {v2}",
    );
    assert!(
        v2["paths"]["/health"]["get"].is_object(),
        "an unversioned route belongs to every document — nothing tells a client \
         to state a version for it",
    );

    assert_eq!(
        app.http().get("/api-json/v9").send().await.0.status(),
        StatusCode::NOT_FOUND,
        "a version nothing declares has no document",
    );
}

#[tokio::test]
async fn the_documented_address_is_the_one_that_answers() {
    let app = TestApp::builder()
        .module::<HeaderVersionedApp>()
        .http(versioned_transport(None))
        .build()
        .await
        .expect("boots");
    let doc = document(&app, "/api-json/v2").await;
    let parameter = version_parameter(&doc["paths"]["/posts"]["get"], DEFAULT_VERSION_HEADER)
        .expect("a version parameter");
    let header = parameter["name"].as_str().expect("a header name");
    let version = parameter["schema"]["enum"][0]
        .as_str()
        .expect("an enumerated version");

    let resp = app
        .http()
        .get("/posts")
        .header(header, version)
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("v2").await;

    assert_eq!(
        app.http().get("/v2/posts").send().await.0.status(),
        StatusCode::NOT_FOUND,
        "the URI form the document no longer names is the one the transport refuses",
    );
}

#[tokio::test]
async fn a_default_version_makes_the_parameter_optional_and_picks_the_document() {
    let app = TestApp::builder()
        .module::<DefaultedApp>()
        .http(versioned_transport(Some("1")))
        .build()
        .await
        .expect("boots");
    let doc = document(&app, "/api-json").await;

    assert_eq!(
        version_parameter(&doc["paths"]["/posts"]["get"], DEFAULT_VERSION_HEADER)
            .expect("a version parameter")["required"],
        false,
        "a caller that states none is served the default: {doc}",
    );
    assert!(
        doc["paths"]["/posts/drafts"].is_null(),
        "and `/api-json` describes that default version, not every version: {doc}",
    );
    let resp = app.http().get("/posts").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v1").await;
}

#[tokio::test]
async fn one_handler_under_two_versions_is_two_operations_a_client_can_tell_apart() {
    let app = TestApp::for_module::<MultiVersionApp>()
        .await
        .expect("boots");
    let doc = document(&app, "/api-json").await;

    assert_eq!(
        operation_id(&doc, "/v1/reports", "get"),
        Some("reports_list_v1"),
    );
    assert_eq!(
        operation_id(&doc, "/v2/reports", "get"),
        Some("reports_list_v2"),
    );
    assert_eq!(
        operation_id(&doc, "/health", "get"),
        Some("health_live"),
        "an unversioned operation is named by its controller alone: {doc}",
    );

    for path in ["/v1/reports", "/v2/reports"] {
        app.http().get(path).send().await.assert_status_is_ok();
    }
}

#[tokio::test]
async fn the_two_controller_layout_names_its_two_versions_apart_as_well() {
    let app = TestApp::builder()
        .module::<HeaderVersionedApp>()
        .http(versioned_transport(None))
        .build()
        .await
        .expect("boots");

    let v1 = document(&app, "/api-json/v1").await;
    let v2 = document(&app, "/api-json/v2").await;
    assert_eq!(operation_id(&v1, "/posts", "get"), Some("posts_v1_list_v1"));
    assert_eq!(operation_id(&v2, "/posts", "get"), Some("posts_v2_list_v2"));
    assert_eq!(
        operation_id(&v1, "/health", "get"),
        Some("health_live"),
        "the unversioned operation is the same one in both documents: {v1}",
    );
    assert_eq!(operation_id(&v2, "/health", "get"), Some("health_live"));
}

#[tokio::test]
async fn a_raw_ident_handler_publishes_an_id_a_client_can_be_generated_from() {
    let app = TestApp::for_module::<RawIdentApp>().await.expect("boots");
    let doc = document(&app, "/api-json").await;

    let id = operation_id(&doc, "/probe/type", "get").expect("the route is documented");
    assert_eq!(id, "probe_r_type");
    assert!(
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "an operationId a generator can name a method after: {id}",
    );
    app.http()
        .get("/probe/type")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn the_uri_strategy_document_does_not_move() {
    let app = TestApp::for_module::<UriVersionedApp>()
        .await
        .expect("boots");
    let doc = document(&app, "/api-json").await;

    assert!(
        doc["paths"]["/v1/posts"]["get"].is_object()
            && doc["paths"]["/v2/posts/drafts"]["get"].is_object(),
        "the version stays in the path: {doc}",
    );
    assert!(
        doc["paths"]["/posts"].is_null(),
        "and the unversioned path is nobody's address: {doc}",
    );
    assert!(
        doc["paths"]["/v1/posts"]["get"].get("parameters").is_none(),
        "no version parameter is added: {doc}",
    );
    assert_eq!(
        app.http().get("/api-json/v1").send().await.0.status(),
        StatusCode::NOT_FOUND,
        "and no second document is mounted",
    );
}
