//! Covers `src/module.rs` — the composition contract `OpenApiModule` publishes.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::poem::web::Multipart;
use nest_rs_http::{
    ApiVersioning, Header, HttpConfig, HttpModule, async_trait, controller, input, routes,
};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule, OpenApiSetup};
use nest_rs_testing::TestApp;
use poem::Request;
use poem::http::StatusCode;
use serde_json::Value;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TokenForm {
    grant_type: String,
}

#[controller(path = "/widgets")]
struct WidgetsController;

#[routes]
impl WidgetsController {
    #[get("/")]
    async fn list(&self) -> String {
        "[]".into()
    }

    #[get("/legacy")]
    #[redirect("/widgets", 301)]
    async fn legacy(&self) {}

    #[get("/feed")]
    #[public]
    #[response_header("cache-control", "no-store")]
    async fn feed(&self) -> String {
        "[]".into()
    }

    #[post("/token")]
    async fn token(&self, body: nest_rs_http::poem::web::Form<TokenForm>) -> String {
        body.0.grant_type
    }

    #[post("/import")]
    #[api(multipart = ImportForm, response_content_type = "text/csv")]
    async fn import(&self, trace: Header<Trace>, form: Multipart) -> String {
        let _ = form;
        trace.into_inner().request_id
    }
}

#[input]
struct Trace {
    #[serde(rename = "X-Request-Id")]
    request_id: String,
    #[serde(rename = "X-Retry-Count")]
    retry: Option<u32>,
}

#[input]
struct ImportForm {
    #[schemars(extend("format" = "binary"))]
    file: String,
}

// Pinned: the unpinned `enabled` default depends on the environment profile.
fn openapi(enabled: bool) -> OpenApiSetup {
    OpenApiModule::for_root(OpenApiConfig {
        enabled,
        title: "Widget API".into(),
        version: "9.9.9".into(),
        // An enabled emit would write `openapi.json` into the runner's directory.
        emit_document: false,
        ..OpenApiConfig::default()
    })
}

#[module(imports = [openapi(true)], providers = [WidgetsController])]
struct DocumentedApp;

#[module(imports = [openapi(false)], providers = [WidgetsController])]
struct UndocumentedApp;

#[tokio::test]
async fn the_documented_import_serves_a_document_describing_the_app() {
    let app = TestApp::for_module::<DocumentedApp>()
        .await
        .expect("importing OpenApiModule is the whole wiring");

    let resp = app.http().get("/api-json").send().await;
    resp.assert_status_is_ok();

    let body = resp.0.into_body().into_bytes().await.expect("a body");
    let doc: Value = serde_json::from_slice(&body).expect("/api-json is JSON");

    // The patch digit follows upstream; `3.1` is the contract.
    let version = doc["openapi"].as_str().expect("an openapi version");
    assert!(
        version.starts_with("3.1."),
        "an OpenAPI 3.1 document, got `{version}`: {doc}",
    );
    assert_eq!(
        doc["info"]["title"], "Widget API",
        "the pinned config reaches the info block",
    );
    assert_eq!(doc["info"]["version"], "9.9.9");
    assert!(
        doc["paths"]["/widgets"]["get"].is_object(),
        "the document describes the controller actually linked in: {doc}",
    );

    let moved = &doc["paths"]["/widgets/legacy"]["get"]["responses"]["301"];
    assert_eq!(
        moved["headers"]["Location"]["schema"]["format"], "uri-reference",
        "the redirect's own header reaches the served document: {moved}",
    );
    assert!(
        doc["paths"]["/widgets"]["get"]["responses"]["200"]
            .get("headers")
            .is_none(),
        "a route that sends no Location declares none: {doc}",
    );
}

#[tokio::test]
async fn a_public_route_states_its_opening_and_the_headers_it_always_sends() {
    let app = TestApp::for_module::<DocumentedApp>().await.expect("boots");
    let resp = app.http().get("/api-json").send().await;
    let body = resp.0.into_body().into_bytes().await.expect("a body");
    let doc: Value = serde_json::from_slice(&body).expect("/api-json is JSON");
    let feed = &doc["paths"]["/widgets/feed"]["get"];

    assert_eq!(
        feed["security"],
        serde_json::json!([]),
        "`#[public]` is an explicit opening in the document too: {feed}",
    );
    let header = &feed["responses"]["200"]["headers"]["cache-control"];
    assert_eq!(header["schema"]["const"], "no-store", "{feed}");

    let served = app.http().get("/widgets/feed").send().await;
    served.assert_header("cache-control", "no-store");
}

#[tokio::test]
async fn the_document_describes_headers_multipart_bodies_and_streamed_responses() {
    let app = TestApp::for_module::<DocumentedApp>().await.expect("boots");
    let resp = app.http().get("/api-json").send().await;
    let body = resp.0.into_body().into_bytes().await.expect("a body");
    let doc: Value = serde_json::from_slice(&body).expect("/api-json is JSON");
    let import = &doc["paths"]["/widgets/import"]["post"];

    let headers: Vec<(&str, bool)> = import["parameters"]
        .as_array()
        .expect("the operation has parameters")
        .iter()
        .filter(|p| p["in"] == "header")
        .map(|p| {
            (
                p["name"].as_str().expect("a name"),
                p["required"].as_bool().expect("a required flag"),
            )
        })
        .collect();
    assert_eq!(
        headers,
        [("X-Request-Id", true), ("X-Retry-Count", false)],
        "the headers the handler reads are documented as it reads them: {import}",
    );
    assert!(
        import["responses"]["400"].is_object(),
        "a required header is a 400 the operation can produce: {import}",
    );

    let form = &import["requestBody"]["content"]["multipart/form-data"]["schema"];
    assert!(
        form["$ref"] == "#/components/schemas/ImportForm" || form.is_object(),
        "the declared parts reach the document: {import}",
    );
    assert_eq!(
        doc["components"]["schemas"]["ImportForm"]["properties"]["file"]["format"], "binary",
        "and a file part is typed as one",
    );

    let ok = &import["responses"]["200"]["content"];
    assert_eq!(
        ok["text/csv"]["schema"]["type"], "string",
        "a declared media type carries a body schema: {import}",
    );
    assert!(
        ok.get("application/json").is_none(),
        "and replaces the JSON default: {import}",
    );
}

#[tokio::test]
async fn the_swagger_ui_and_its_assets_are_served() {
    let app = TestApp::for_module::<DocumentedApp>().await.expect("boots");

    app.http().get("/api").send().await.assert_status_is_ok();
    app.http()
        .get("/api/swagger-ui-bundle.js")
        .send()
        .await
        .assert_status_is_ok();
    app.http()
        .get("/api/swagger-ui.css")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn disabled_serves_neither_endpoint() {
    let app = TestApp::for_module::<UndocumentedApp>()
        .await
        .expect("a disabled module still boots — it is an opt-out, not an error");

    for path in ["/api-json", "/api", "/api/swagger-ui-bundle.js"] {
        assert_eq!(
            app.http().get(path).send().await.0.status(),
            StatusCode::NOT_FOUND,
            "`{path}` must not answer when the documentation is disabled",
        );
    }
}

#[module(
    imports = [
        openapi(true),
        HttpModule::for_root(HttpConfig {
            versioning: ApiVersioning::Header,
            default_version: Some("9".into()),
            ..HttpConfig::default()
        }),
    ],
    providers = [WidgetsController],
)]
struct MisversionedApp;

#[tokio::test]
async fn a_default_version_nothing_declares_fails_the_boot() {
    let err = TestApp::for_module::<MisversionedApp>()
        .await
        .err()
        .expect("a default version no controller declares is a boot failure")
        .to_string();
    assert!(
        err.contains(&nest_rs_config::var_name("http", "DEFAULT_VERSION")),
        "the boot failure names the variable to change: {err}",
    );
    assert!(
        err.contains("#[controller(version"),
        "and the decorator that would declare it: {err}",
    );
}

#[injectable]
#[derive(Default)]
struct DenyEveryone;

impl Layer for DenyEveryone {}

#[async_trait]
impl Guard for DenyEveryone {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::unauthorized("no"))
    }
}

impl HttpGuard for DenyEveryone {}

#[module(imports = [openapi(true)], providers = [WidgetsController, DenyEveryone])]
struct GuardedApp;

#[tokio::test]
async fn the_documentation_endpoints_ignore_the_global_guard_chain() {
    let app = TestApp::builder()
        .module::<GuardedApp>()
        .use_guards_global([guard::<DenyEveryone>()])
        .build()
        .await
        .expect("boots");

    app.http()
        .get("/widgets")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    app.http()
        .get("/api-json")
        .send()
        .await
        .assert_status_is_ok();
}

#[controller(path = "/", version = "1")]
struct RootCatchAllController;

#[routes]
impl RootCatchAllController {
    #[get("/*rest")]
    async fn anything(&self) -> String {
        "root-catch-all".into()
    }
}

#[module(
    imports = [
        openapi(true),
        HttpModule::for_root(HttpConfig {
            versioning: ApiVersioning::Header,
            default_version: Some("1".into()),
            ..HttpConfig::default()
        }),
    ],
    providers = [RootCatchAllController],
)]
struct CatchAllApp;

#[tokio::test]
async fn a_versioned_root_catch_all_does_not_swallow_the_documents_own_addresses() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<CatchAllApp>()
        .await
        .expect("boots with a versioned root controller beside the documentation");

    let omitted = logs.find(
        "nest_rs::openapi",
        "route omitted from the document: an OpenAPI path template is one whole \
         segment, so a catch-all, an unnamed pattern, or a literal sharing a \
         segment with a parameter cannot be described",
    );
    assert!(
        !omitted.is_empty(),
        "the catch-all is reported: {:#?}",
        logs.events()
    );
    for event in &omitted {
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("handler").as_deref(), Some("anything"));
        assert!(
            event.field("path").is_some_and(|p| p.contains("*rest")),
            "the event names the path it could not template, got {:?}",
            event.fields,
        );
    }

    for path in ["/api", "/api-json", "/api/swagger-ui.css"] {
        let resp = app.http().get(path).send().await;
        let status = resp.0.status();
        let body = resp.0.into_body().into_string().await.unwrap_or_default();
        assert_eq!(status, StatusCode::OK, "`{path}` still answers");
        assert!(
            !body.contains("root-catch-all"),
            "`{path}` is the documentation's, not the catch-all's: {body}",
        );
    }

    let resp = app.http().get("/anything-else").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("root-catch-all").await;
}

#[tokio::test]
async fn a_form_encoded_body_is_described_as_one() {
    let app = TestApp::for_module::<DocumentedApp>().await.expect("boots");
    let document: Value = app
        .http()
        .get("/api-json")
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();

    let body = &document["paths"]["/widgets/token"]["post"]["requestBody"];
    assert!(
        !body.is_null(),
        "a form-encoded route declares a body: {}",
        serde_json::to_string_pretty(&document["paths"]["/widgets/token"]).unwrap_or_default(),
    );
    let content = &body["content"]["application/x-www-form-urlencoded"];
    assert!(
        !content.is_null(),
        "under the media type the wire actually carries: {body}",
    );
    assert!(
        content["schema"]["$ref"].is_string() || content["schema"]["properties"].is_object(),
        "and carrying the form's own shape: {content}",
    );
}

#[module(
    imports = [
        OpenApiModule::for_root(OpenApiConfig {
            emit_document: true,
            document_path: unwritable_document_path(),
            ..OpenApiConfig::default()
        }),
    ],
    providers = [WidgetsController],
)]
struct UnwritableDocumentApp;

fn unwritable_document_path() -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("nest_rs_openapi_absent_{}", std::process::id()))
        .join("openapi.json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_emit_that_cannot_write_warns_and_still_serves() {
    // Global: the write runs on a `spawn_blocking` thread a thread-local capture cannot see.
    let logs = nest_rs_testing::LogCapture::install_global();

    let app = TestApp::for_module::<UnwritableDocumentApp>()
        .await
        .expect("a failed document write is never a boot failure");

    let resp = app.http().get("/api-json").send().await;
    resp.assert_status_is_ok();

    for _ in 0..200 {
        if !logs
            .find(nest_rs_openapi::TARGET, "failed to write OpenAPI document")
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let event = logs.expect_one(nest_rs_openapi::TARGET, "failed to write OpenAPI document");
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("path").as_deref(),
        unwritable_document_path().to_str(),
        "the event names the file that did not get written — the whole path, \
         since `openapi.json` is what every one of them ends with: {:?}",
        event.fields,
    );
    assert!(
        event.field("error").is_some(),
        "and why, got {:?}",
        event.fields,
    );
    assert!(
        logs.find(nest_rs_openapi::TARGET, "wrote OpenAPI document")
            .is_empty(),
        "the success line and the failure line are exclusive: {:#?}",
        logs.events(),
    );
}
