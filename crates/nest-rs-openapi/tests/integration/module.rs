//! Covers `src/module.rs` — the composition contract `OpenApiModule` publishes.
//!
//! One import, no `main.rs` wiring, two endpoints. Every assertion here is a
//! sentence the module's own documentation makes, checked against a booted app
//! rather than against the mount table: the document describes the routes that
//! are actually linked in, `enabled = false` serves nothing, and the endpoints
//! are reachable **without authentication** — the exposure that makes the
//! disable switch matter in production.

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

/// A route with something to document, so "the document describes the app" is
/// an observation rather than an assumption about an empty `paths` object.
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

    /// A route whose response *is* a header. `#[crud]`'s create is the other
    /// producer of `Location`, and it needs an entity and a live database — this
    /// one exercises the same `sets_location` path from decorator to served
    /// document with neither.
    #[get("/legacy")]
    #[redirect("/widgets", 301)]
    async fn legacy(&self) {}

    /// The three shapes a document used to say nothing about: the headers a
    /// handler reads, a `multipart/form-data` body, and a success body that is
    /// not JSON. All three are decorator-to-document paths, so a unit test on
    /// the composer proves only half of each.
    /// A form-encoded body — the shape RFC 6749 requires of an OAuth token
    /// endpoint, and the one `request_body` recognised neither as a body nor as
    /// an error: the operation was published with no `requestBody` at all.
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

/// A required header and an optional one, so the served document has both
/// `required` answers to show.
#[input]
struct Trace {
    #[serde(rename = "X-Request-Id")]
    request_id: String,
    #[serde(rename = "X-Retry-Count")]
    retry: Option<u32>,
}

/// The parts of the `multipart/form-data` body `import` reads — the form's
/// shape is declared, because no extractor states it.
#[input]
struct ImportForm {
    #[schemars(extend("format" = "binary"))]
    file: String,
}

/// The config is **pinned** rather than read from `NESTRS_OPENAPI__*`: the
/// unpinned default is deliberately environment-dependent (off outside a dev
/// profile), so a suite that let the ambient environment decide would assert
/// one thing on a laptop and another on a build agent.
fn openapi(enabled: bool) -> OpenApiSetup {
    OpenApiModule::for_root(OpenApiConfig {
        enabled,
        title: "Widget API".into(),
        version: "9.9.9".into(),
        // Left off explicitly: an enabled emit would write `openapi.json` into
        // whatever directory the test runner happens to be in.
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

    // `3.1.x`, not an exact string: the patch digit tracks the spec revision
    // schemars emits, and pinning it would turn a harmless upstream bump into a
    // failing suite. The `3.1` line is the contract — a `3.0` document would be
    // a different format for every consumer.
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

    // R10: a header the framework knows it sends must be declared, or the
    // generated client never reads it — the gap the throttler's `Retry-After`
    // had already closed for its `429`, left open on the success side.
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
async fn the_document_describes_headers_multipart_bodies_and_streamed_responses() {
    let app = TestApp::for_module::<DocumentedApp>().await.expect("boots");
    let resp = app.http().get("/api-json").send().await;
    let body = resp.0.into_body().into_bytes().await.expect("a body");
    let doc: Value = serde_json::from_slice(&body).expect("/api-json is JSON");
    let import = &doc["paths"]["/widgets/import"]["post"];

    // One `in: header` parameter per property of the DTO, `required` read off
    // the schema — an `Option<_>` field is a header the caller may omit.
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

    // The body arrives as a form, and the form's parts are typed.
    let form = &import["requestBody"]["content"]["multipart/form-data"]["schema"];
    assert!(
        form["$ref"] == "#/components/schemas/ImportForm" || form.is_object(),
        "the declared parts reach the document: {import}",
    );
    assert_eq!(
        doc["components"]["schemas"]["ImportForm"]["properties"]["file"]["format"], "binary",
        "and a file part is typed as one",
    );

    // The success body is not JSON, and the document says what it is.
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
    // The UI is only useful if its bundled assets resolve; they hang off `/api/`
    // *relative* to the docs path, which is why the two must stay siblings.
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
    // The production posture. The unit tests prove the mount table is empty;
    // this proves the consequence a deployment actually cares about — that no
    // anonymous caller can read the schema.
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
    // The default version is what `/api-json` describes under a non-URI
    // strategy, so one nothing declares publishes an empty document — a wiring
    // mistake that reads to every client as "this deployment serves nothing".
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

/// Refuses every caller, registered globally — the strictest posture an app can
/// declare.
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
    // `EdgePosture::Exempt`, stated as a consequence rather than a comment: an
    // app that denies every request still publishes its full schema to anyone.
    // That is the documented behaviour and the whole reason `enabled = false`
    // exists — if this ever starts returning 401, the disable switch stopped
    // being the only thing standing between a schema and the public.
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

/// A versioned controller mounted at the root, which is the shape that reaches
/// every address in the app.
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

/// The document's own address is not the module's mount path.
///
/// `OpenApiModule` nests at `/api` and serves `/api-json` — a **sibling**, not a
/// child. The version selector's neutrality list was built as "the endpoint's
/// path, plus its subtree", so `/api-json` was in neither, and a versioned root
/// catch-all with a deployment default answered the document's address with its
/// own body: `200`, wrong payload, nothing logged.
///
/// Every path the module registers is now declared, so this asks the module
/// rather than inferring from one corner of it.
#[tokio::test]
async fn a_versioned_root_catch_all_does_not_swallow_the_documents_own_addresses() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<CatchAllApp>()
        .await
        .expect("boots with a versioned root controller beside the documentation");

    // The same catch-all is a path OpenAPI has no template for, so it is left
    // out of the document rather than published verbatim — a generated client
    // once called `/blobs/*rest` as a literal URL. The route still serves, so
    // nothing about the omission is observable from the app; the warn is the
    // whole notice the author gets.
    // Two: the document is rendered per selected version, and the catch-all is
    // omitted from each. One line per rendering is the honest count — the
    // author has to fix one route either way.
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

    // And the catch-all still serves everything that is genuinely its own.
    let resp = app.http().get("/anything-else").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("root-catch-all").await;
}

/// `Form<T>` is a request body, and the document said the route had none.
///
/// `request_body` matched `Json<T>`, `#[api(multipart = T)]` and a bare
/// `Multipart` parameter; a form-encoded body matched nothing and there was no
/// refusal either, so the operation reached a generated client as a `POST` with
/// nothing to send.
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

/// A committed `openapi.json` refreshed as a side effect of a run — the same
/// dev-loop convenience the GraphQL SDL emit is — pointed at a directory that
/// does not exist.
///
/// It must not stop a boot: the app serves the document at `/api-json` whether
/// or not the file was written, and an operator running in a read-only image
/// wants the app up. Which leaves nothing else to notice that the committed
/// document has silently stopped tracking the routes — every client generated
/// from it drifts from the app one release at a time.
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
    // Global: the write is offloaded to `spawn_blocking` so a synchronous write
    // never stalls the boot executor, which puts the event on another thread —
    // the one shape a thread-local capture is structurally blind to.
    let logs = nest_rs_testing::LogCapture::install_global();

    let app = TestApp::for_module::<UnwritableDocumentApp>()
        .await
        .expect("a failed document write is never a boot failure");

    let resp = app.http().get("/api-json").send().await;
    resp.assert_status_is_ok();

    // The blocking pool runs on its own threads; give it a moment to land.
    for _ in 0..200 {
        if !logs
            .find("nest_rs::routes", "failed to write OpenAPI document")
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let event = logs.expect_one("nest_rs::routes", "failed to write OpenAPI document");
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
        logs.find("nest_rs::routes", "wrote OpenAPI document")
            .is_empty(),
        "the success line and the failure line are exclusive: {:#?}",
        logs.events(),
    );
}
