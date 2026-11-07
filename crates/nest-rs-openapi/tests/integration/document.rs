//! Covers `src/document.rs` — what the served document says about a version, and
//! what a route answers, each status read live beside it.

use nest_rs_config::Surface;
use nest_rs_core::module;
use nest_rs_http::StatusCode;
use nest_rs_http::{
    ApiVersioning, DEFAULT_VERSION_HEADER, HttpConfig, HttpModule, controller, routes,
};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule, OpenApiSetup};
use nest_rs_testing::TestApp;
use serde_json::Value;

#[controller(path = "/posts", version = "1")]
struct PostsV1Controller;

#[routes]
impl PostsV1Controller {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        "v1".into()
    }
}

#[controller(path = "/posts", version = "2")]
struct PostsV2Controller;

#[routes]
impl PostsV2Controller {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        "v2".into()
    }

    #[get("/drafts")]
    #[public]
    async fn drafts(&self) -> String {
        "[]".into()
    }
}

#[controller(path = "/reports", version = ["1", "2"])]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        "reports".into()
    }
}

#[controller(path = "/probe")]
struct ProbeController;

#[routes]
impl ProbeController {
    #[get("/type")]
    #[public]
    async fn r#type(&self) -> String {
        "raw".into()
    }
}

#[controller(path = "/health")]
struct HealthController;

#[routes]
impl HealthController {
    #[get("/")]
    #[public]
    async fn live(&self) -> String {
        "ok".into()
    }
}

fn openapi() -> OpenApiSetup {
    OpenApiModule::for_root(OpenApiConfig {
        enabled: Surface::Open,
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

/// The document names the app as its binary does: the tested package's name,
/// version and description, which the harness publishes as `#[nest_rs::main]`
/// publishes the binary's.
#[tokio::test]
async fn the_document_names_the_app_by_its_package() {
    let app = TestApp::for_module::<RawIdentApp>().await.expect("boots");
    let doc = document(&app, "/api-json").await;

    assert_eq!(
        doc["info"]["title"],
        env!("CARGO_PKG_NAME"),
        "{}",
        doc["info"]
    );
    assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(doc["info"]["description"], env!("CARGO_PKG_DESCRIPTION"));
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

/// A route lists what its guards declare they refuse a request with — the
/// global pool's, its controller's and its own — whatever the guard is named.
mod guards {
    use nest_rs_authn::{AuthError, AuthnGuard, PrincipalIdentity, Strategy};
    use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Read, subject};
    use nest_rs_core::async_trait;
    use nest_rs_core::{
        Ambient, AmbientValue, ChallengeScheme, DenialDocument, Layer, UnitContext, injectable,
        module,
    };
    use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};
    use nest_rs_http::{HttpView, Request, controller, routes};
    use nest_rs_testing::TestApp;
    use nest_rs_throttler::ThrottlerGuard as Limit;
    use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule};
    use serde_json::Value;

    use super::{document, openapi};

    /// Refuses a caller outside the office network, `#[public]` routes
    /// included.
    #[injectable]
    #[derive(Default)]
    struct OfficeOnly;

    impl Layer for OfficeOnly {}

    #[async_trait]
    impl Guard for OfficeOnly {
        async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Err(Denial::forbidden("outside the office network"))
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }

        fn denial_statuses(&self, _public: bool) -> &'static [u16] {
            &[403]
        }
    }

    impl GuardFor<HttpView> for OfficeOnly {}

    #[controller(path = "/rated")]
    struct RatedController;

    #[routes]
    impl RatedController {
        #[get("/")]
        #[public]
        async fn list(&self) -> &'static str {
            "[]"
        }

        #[get("/feed")]
        #[public]
        async fn feed(&self) -> &'static str {
            "[]"
        }

        #[get("/office")]
        #[public]
        #[use_guards(OfficeOnly)]
        async fn office(&self) -> &'static str {
            "[]"
        }
    }

    #[module(
        imports = [openapi(), ThrottlerModule],
        providers = [OfficeOnly, RatedController],
    )]
    struct RatedApp;

    #[module(imports = [RatedApp], providers = [nest_rs_guards::GlobalGuards<(ThrottlerGuard,)>])]
    struct ThrottledGloballyApp;

    /// The throttler as the rate-limiting page binds it: in the pool alone.
    async fn throttled_globally() -> Value {
        let app = TestApp::builder()
            .module::<ThrottledGloballyApp>()
            .build()
            .await
            .expect("importing ThrottlerModule provides the guard the pool names");
        document(&app, "/api-json").await
    }

    #[controller(path = "/limited")]
    struct LimitedController;

    #[routes]
    impl LimitedController {
        #[get("/")]
        #[public]
        #[use_guards(Limit)]
        async fn list(&self) -> &'static str {
            "[]"
        }
    }

    #[module(
        imports = [openapi(), ThrottlerModule],
        providers = [LimitedController],
    )]
    struct LimitedApp;

    #[derive(Clone)]
    struct Caller;

    impl AmbientValue for Caller {}

    impl PrincipalIdentity for Caller {
        fn actor_id(&self) -> Option<String> {
            Some("caller".into())
        }
    }

    #[injectable]
    #[derive(Default)]
    struct Bearer;

    #[async_trait]
    impl Strategy for Bearer {
        type Principal = Caller;

        async fn authenticate(&self, req: &mut Request) -> Result<Caller, AuthError> {
            match req.header("authorization") {
                Some(_) => Err(AuthError::InvalidToken),
                None => Err(AuthError::MissingCredentials),
            }
        }
    }

    type Authn = AuthnGuard<Bearer>;

    /// Decides on the principal an earlier guard attached, and requires it.
    #[injectable]
    #[derive(Default)]
    struct OwnAccount;

    impl Layer for OwnAccount {}

    #[async_trait]
    impl Guard for OwnAccount {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authorization
        }

        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<Caller>())
        }

        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            cx.require::<Caller>()?;
            Ok(())
        }
    }

    impl GuardFor<HttpView> for OwnAccount {}

    #[controller(path = "/accounts")]
    #[use_guards(Authn)]
    struct AccountsController;

    #[routes]
    impl AccountsController {
        #[get("/")]
        async fn list(&self) -> &'static str {
            "[]"
        }

        #[get("/signup")]
        #[public]
        async fn signup(&self) -> &'static str {
            "{}"
        }

        #[get("/me")]
        #[public]
        async fn me(&self, _caller: Ambient<Caller>) -> &'static str {
            "{}"
        }

        #[get("/own")]
        #[public]
        #[use_guards(OwnAccount)]
        async fn own(&self) -> &'static str {
            "{}"
        }
    }

    #[module(imports = [openapi()], providers = [Bearer, Authn, OwnAccount, AccountsController])]
    struct AccountsApp;

    /// Challenges `Basic` in its own words, as a token endpoint's client guard does.
    #[injectable]
    #[derive(Default)]
    struct BasicClients;

    impl Layer for BasicClients {}

    #[async_trait]
    impl Guard for BasicClients {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }

        fn denial_statuses(&self, _public: bool) -> &'static [u16] {
            &[401]
        }

        fn denial_document(&self) -> DenialDocument {
            DenialDocument::answering(ChallengeScheme::Basic, |generator| {
                generator.subschema_for::<String>()
            })
        }
    }

    impl GuardFor<HttpView> for BasicClients {}

    #[subject]
    struct Browsing;

    /// The rules the gate reads; a document runs none of them.
    #[injectable]
    #[derive(Default)]
    struct BrowsingAbility;

    impl AbilityFactory for BrowsingAbility {
        type Actor = Caller;

        fn define(&self, _actor: &Caller, _ability: &mut AbilityBuilder) {}
    }

    type Authz = AbilityGuard<BrowsingAbility>;

    #[controller(path = "/machines")]
    #[use_guards(BasicClients, Authz)]
    struct MachinesController;

    #[routes]
    impl MachinesController {
        #[get("/")]
        #[authorize(Read, Browsing)]
        async fn list(&self) -> &'static str {
            "[]"
        }
    }

    #[module(
        imports = [openapi()],
        providers = [BasicClients, BrowsingAbility, Authz, MachinesController],
    )]
    struct MachinesApp;

    fn responses<'d>(document: &'d Value, path: &str) -> &'d Value {
        &document["paths"][path]["get"]["responses"]
    }

    fn assert_the_wait_is_declared(response: &Value) {
        assert_eq!(
            response["headers"]["Retry-After"]["schema"]["type"], "integer",
            "the status declares the Retry-After it carries: {response}",
        );
    }

    /// A class gate's `401` is reached past the chain's, so it offers no scheme
    /// the chain does not challenge with: what its `403` adds stays its own.
    #[tokio::test]
    async fn a_class_gate_adds_no_scheme_beside_a_guard_challenging_its_own() {
        let app = TestApp::for_module::<MachinesApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        assert_eq!(
            doc["paths"]["/machines"]["get"]["security"],
            serde_json::json!([{ "basicAuth": [] }]),
        );
        let responses = responses(&doc, "/machines");
        let refused = &responses["401"];
        assert!(
            refused["content"].get("application/problem+json").is_none(),
            "{refused}",
        );
        let challenge = refused["headers"]["WWW-Authenticate"]["description"]
            .as_str()
            .expect("a 401 names its challenge");
        assert!(!challenge.contains("Bearer"), "{challenge}");
        assert!(
            responses["403"]["content"]
                .get("application/problem+json")
                .is_some(),
            "{responses}",
        );
    }

    #[tokio::test]
    async fn a_route_behind_the_global_throttler_documents_its_429_and_the_wait() {
        let doc = throttled_globally().await;
        let responses = responses(&doc, "/rated");
        assert_the_wait_is_declared(&responses["429"]);
        for status in ["401", "403"] {
            assert!(
                responses.get(status).is_none(),
                "a throttler refuses no credential, so no {status}: {responses}",
            );
        }
    }

    #[tokio::test]
    async fn a_public_route_under_the_global_throttler_documents_its_429_and_no_401() {
        let doc = throttled_globally().await;
        let responses = responses(&doc, "/rated/feed");
        assert_the_wait_is_declared(&responses["429"]);
        assert!(responses.get("401").is_none(), "{responses}");
        assert_eq!(
            doc["paths"]["/rated/feed"]["get"]["security"],
            Value::Array(Vec::new())
        );
    }

    #[tokio::test]
    async fn an_aliased_throttler_documents_its_429() {
        let app = TestApp::for_module::<LimitedApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        assert_the_wait_is_declared(&responses(&doc, "/limited")["429"]);
    }

    #[tokio::test]
    async fn a_guard_s_own_declaration_reaches_the_document_on_a_public_route() {
        let doc = throttled_globally().await;
        let responses = responses(&doc, "/rated/office");
        assert_eq!(responses["403"]["description"], "Forbidden", "{responses}");
        assert!(
            responses.get("429").is_some() && responses.get("401").is_none(),
            "the pool's declaration beside the route's own: {responses}",
        );
    }

    #[tokio::test]
    async fn an_authenticated_route_documents_the_strategy_that_could_not_answer() {
        let app = TestApp::for_module::<AccountsApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let gated = responses(&doc, "/accounts");
        assert_eq!(
            gated["401"]["headers"]["WWW-Authenticate"]["required"], true,
            "{gated}"
        );
        assert_the_wait_is_declared(&gated["503"]);
        assert!(
            gated.get("403").is_none(),
            "an authentication guard decides no authorization: {gated}",
        );

        let public = responses(&doc, "/accounts/signup");
        assert_the_wait_is_declared(&public["503"]);
        assert!(public.get("401").is_none(), "{public}");
    }

    /// A `#[public]` route whose handler needs the principal its chain's guard
    /// attaches admits no caller that guard did not identify: a rejected token
    /// earns its `401`, a caller with none the caller's `401`, and the document
    /// says both, with the scheme the route requires.
    #[tokio::test]
    async fn a_public_route_needing_its_principal_answers_and_documents_its_401() {
        let app = TestApp::for_module::<AccountsApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let me = responses(&doc, "/accounts/me");
        assert_eq!(
            me["401"]["headers"]["WWW-Authenticate"]["required"], true,
            "{me}"
        );
        assert_eq!(
            doc["paths"]["/accounts/me"]["get"]["security"],
            serde_json::json!([{ "bearerAuth": [] }]),
        );

        let anonymous = app.http().get("/accounts/me").send().await;
        anonymous.assert_status(nest_rs_http::StatusCode::UNAUTHORIZED);
        anonymous.assert_header("www-authenticate", "Bearer");
        let rejected = app
            .http()
            .get("/accounts/me")
            .header("authorization", "Bearer forged")
            .send()
            .await;
        rejected.assert_status(nest_rs_http::StatusCode::UNAUTHORIZED);
        rejected.assert_header("www-authenticate", r#"Bearer error="invalid_token""#);
    }

    /// The same on a `#[public]` route whose later guard requires that principal:
    /// the guard's absence is the caller's `401`, so the document says it.
    #[tokio::test]
    async fn a_public_route_whose_guard_requires_its_principal_documents_its_401() {
        let app = TestApp::for_module::<AccountsApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let own = responses(&doc, "/accounts/own");
        assert_eq!(
            own["401"]["headers"]["WWW-Authenticate"]["required"], true,
            "{own}"
        );

        app.http()
            .get("/accounts/own")
            .send()
            .await
            .assert_status(nest_rs_http::StatusCode::UNAUTHORIZED);
    }
}

/// What a route answers, read off a live request, beside what its document says.
mod answers {
    use std::collections::HashMap;
    use std::time::Duration;

    use futures_util::StreamExt as _;
    use nest_rs_core::{input, module, output};
    use nest_rs_http::{
        ApiVersioning, Body, Bytes, Created, Form, Header, HttpConfig, HttpCors, HttpModule,
        IntoResponse, Json, Multipart, Paginated, Path, ProblemDetails, Query, RawBody, Response,
        Result, StatusCode, controller, header, routes,
    };
    use nest_rs_testing::{TestApp, TestClient};
    use serde_json::{Value, json};

    use super::{document, openapi};

    const JSON_BODY: &str = "application/json";
    const FORM_BODY: &str = "application/x-www-form-urlencoded";
    const MULTIPART_BODY: &str = "multipart/form-data; boundary=x";

    #[input]
    struct Order {
        sku: String,
    }

    #[output]
    struct Receipt {
        id: u32,
    }

    fn receipt() -> Json<Receipt> {
        Json(Receipt { id: 7 })
    }

    #[input]
    struct Limits {
        limit: Option<u32>,
    }

    #[input]
    struct Retried {
        #[serde(rename = "X-Retried")]
        retried: Option<u32>,
    }

    /// An answer of the application's own, whose status only it knows.
    struct Teapot;

    impl IntoResponse for Teapot {
        fn into_response(self) -> Response {
            StatusCode::IM_A_TEAPOT.into_response()
        }
    }

    type Queued = (StatusCode, Json<Receipt>);

    #[controller(path = "/orders")]
    struct OrdersController;

    #[routes]
    impl OrdersController {
        #[get("/slow")]
        #[public]
        async fn slow(&self) -> &'static str {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            "late"
        }

        #[post("/json")]
        #[public]
        async fn json(&self, Json(order): Json<Order>) -> String {
            order.sku
        }

        #[post("/form")]
        #[public]
        async fn form(&self, Form(order): Form<Order>) -> String {
            order.sku
        }

        #[post("/upload")]
        #[public]
        async fn upload(&self, mut parts: Multipart) -> Result<&'static str> {
            while let Some(part) = parts.next_field().await? {
                drop(part.bytes().await?);
            }
            Ok("taken")
        }

        #[post("/raw")]
        #[public]
        async fn raw(&self, body: RawBody) -> String {
            body.0.len().to_string()
        }

        #[post("/stream")]
        #[public]
        async fn stream(&self, body: Body) -> Result<String> {
            Ok(body.into_bytes().await?.len().to_string())
        }

        #[post("/pair")]
        #[public]
        async fn pair(&self) -> (StatusCode, Json<Receipt>) {
            (StatusCode::ACCEPTED, receipt())
        }

        #[post("/result-pair")]
        #[public]
        async fn result_pair(&self) -> Result<Queued> {
            Ok((StatusCode::ACCEPTED, receipt()))
        }

        #[post("/text-pair")]
        #[public]
        async fn text_pair(&self) -> (StatusCode, &'static str) {
            (StatusCode::ACCEPTED, "queued")
        }

        #[post("/pinned-pair")]
        #[public]
        #[http_code(202)]
        async fn pinned_pair(&self) -> (StatusCode, Json<Receipt>) {
            (StatusCode::CREATED, receipt())
        }

        #[delete("/status")]
        #[public]
        async fn status(&self) -> StatusCode {
            StatusCode::NO_CONTENT
        }

        #[delete("/result-status")]
        #[public]
        async fn result_status(&self) -> Result<StatusCode> {
            Ok(StatusCode::NO_CONTENT)
        }

        #[post("/built")]
        #[public]
        #[api(response = Receipt)]
        async fn built(&self) -> Response {
            Response::builder().status(StatusCode::ACCEPTED).body("{}")
        }

        #[post("/own")]
        #[public]
        async fn own(&self) -> Teapot {
            Teapot
        }

        #[get("/plain")]
        #[public]
        async fn plain(&self) -> Json<Receipt> {
            receipt()
        }

        #[delete("/nothing")]
        #[public]
        async fn nothing(&self) {}

        #[get("/item/{name}")]
        #[public]
        async fn item(&self, Path(name): Path<String>) -> String {
            name
        }

        #[get("/numbered/{id}")]
        #[public]
        async fn numbered(&self, Path(id): Path<u32>) -> String {
            id.to_string()
        }

        #[get("/filtered")]
        #[public]
        async fn filtered(&self, Query(limits): Query<Limits>) -> String {
            format!("{:?}", limits.limit)
        }

        #[get("/retried")]
        #[public]
        async fn retried(&self, head: Header<Retried>) -> String {
            format!("{:?}", head.0.retried)
        }

        #[get("/form-query")]
        #[public]
        async fn form_query(&self, Form(order): Form<Order>) -> String {
            order.sku
        }

        #[get("/free-query")]
        #[public]
        async fn free_query(&self, Query(free): Query<HashMap<String, u32>>) -> String {
            free.len().to_string()
        }

        #[get("/free-form-query")]
        #[public]
        async fn free_form_query(&self, Form(free): Form<HashMap<String, u32>>) -> String {
            free.len().to_string()
        }
    }

    /// One route per spelling writing a body no schema names.
    #[controller(path = "/bodies")]
    struct BodiesController;

    #[routes]
    impl BodiesController {
        #[get("/string")]
        #[public]
        async fn string(&self) -> String {
            "text".to_owned()
        }

        #[get("/str")]
        #[public]
        async fn text(&self) -> &'static str {
            "text"
        }

        #[get("/bytes")]
        #[public]
        async fn bytes(&self) -> Bytes {
            Bytes::from_static(b"octets")
        }

        #[get("/vec")]
        #[public]
        async fn vec(&self) -> Vec<u8> {
            b"octets".to_vec()
        }

        #[get("/body")]
        #[public]
        async fn body(&self) -> Body {
            Body::from("whatever")
        }

        #[get("/result")]
        #[public]
        async fn result(&self) -> Result<String> {
            Ok("text".to_owned())
        }

        #[get("/pair/string")]
        #[public]
        async fn pair_string(&self) -> (StatusCode, String) {
            (StatusCode::ACCEPTED, "text".to_owned())
        }

        #[get("/pair/str")]
        #[public]
        async fn pair_text(&self) -> (StatusCode, &'static str) {
            (StatusCode::ACCEPTED, "text")
        }

        #[get("/pair/bytes")]
        #[public]
        async fn pair_bytes(&self) -> (StatusCode, Bytes) {
            (StatusCode::ACCEPTED, Bytes::from_static(b"octets"))
        }

        #[get("/pair/vec")]
        #[public]
        async fn pair_vec(&self) -> (StatusCode, Vec<u8>) {
            (StatusCode::ACCEPTED, b"octets".to_vec())
        }

        #[get("/pair/body")]
        #[public]
        async fn pair_body(&self) -> (StatusCode, Body) {
            (StatusCode::ACCEPTED, Body::from("whatever"))
        }

        #[get("/pair/result")]
        #[public]
        async fn pair_result(&self) -> Result<(StatusCode, Bytes)> {
            Ok((StatusCode::ACCEPTED, Bytes::from_static(b"octets")))
        }

        #[get("/created")]
        #[public]
        async fn created(&self) -> Created<String> {
            Created::at(7, "text".to_owned())
        }

        #[get("/paginated")]
        #[public]
        async fn paginated(&self) -> Result<Paginated<Bytes>> {
            Ok(Paginated::new(Bytes::from_static(b"octets"), 2, Some(2)))
        }

        #[get("/pair/created")]
        #[public]
        async fn pair_created(&self) -> (StatusCode, Created<Bytes>) {
            (
                StatusCode::ACCEPTED,
                Created::at(7, Bytes::from_static(b"octets")),
            )
        }

        #[get("/html")]
        #[public]
        #[response_header("content-type", "text/html; charset=utf-8")]
        async fn html(&self) -> String {
            "<p>text</p>".to_owned()
        }

        #[get("/vendored")]
        #[public]
        #[response_header("content-type", "application/vnd.api+json")]
        async fn vendored(&self) -> Json<Receipt> {
            receipt()
        }

        #[get("/declared")]
        #[public]
        #[api(response_content_type = "text/csv")]
        async fn declared(&self) -> Response {
            Response::builder()
                .content_type(nest_rs_http::HeaderValue::from_static("text/csv"))
                .body("a,b")
        }

        #[get("/problem")]
        #[public]
        async fn problem(&self) -> ProblemDetails {
            ProblemDetails::not_found()
        }

        #[get("/problem/result")]
        #[public]
        async fn problem_result(&self) -> Result<ProblemDetails> {
            Ok(ProblemDetails::from_status(StatusCode::SERVICE_UNAVAILABLE))
        }
    }

    #[module(imports = [openapi()], providers = [OrdersController, BodiesController])]
    struct DefaultApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig { request_timeout: None, ..HttpConfig::default() }),
        ],
        providers = [OrdersController],
    )]
    struct UnboundedApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig {
                request_timeout: Some(Duration::from_secs(5)),
                body_read_timeout: Duration::from_secs(6),
                ..HttpConfig::default()
            }),
        ],
        providers = [OrdersController],
    )]
    struct GapPastTheBudgetApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig { max_body_bytes: 8, ..HttpConfig::default() }),
        ],
        providers = [OrdersController],
    )]
    struct TightCapApp;

    fn cors(origins: &[&str]) -> Option<HttpCors> {
        Some(HttpCors {
            origins: origins.iter().map(|origin| (*origin).to_owned()).collect(),
            ..HttpCors::default()
        })
    }

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig {
                cors: cors(&["https://app.example"]),
                ..HttpConfig::default()
            }),
        ],
        providers = [OrdersController],
    )]
    struct ListedOriginsApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig { cors: cors(&["*"]), ..HttpConfig::default() }),
        ],
        providers = [OrdersController],
    )]
    struct EveryOriginApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig {
                versioning: ApiVersioning::Header,
                default_version: Some("1".into()),
                ..HttpConfig::default()
            }),
        ],
        providers = [OrdersController, super::PostsV1Controller, super::PostsV2Controller],
    )]
    struct HeaderVersionedApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig {
                versioning: ApiVersioning::MediaType,
                default_version: Some("1".into()),
                ..HttpConfig::default()
            }),
        ],
        providers = [OrdersController, super::PostsV1Controller],
    )]
    struct MediaTypeVersionedApp;

    #[module(
        imports = [
            openapi(),
            HttpModule::for_root(HttpConfig {
                versioning: ApiVersioning::Header,
                ..HttpConfig::default()
            }),
        ],
        providers = [OrdersController],
    )]
    struct NothingVersionedApp;

    fn responses<'d>(document: &'d Value, path: &str, method: &str) -> &'d Value {
        &document["paths"][path][method]["responses"]
    }

    #[track_caller]
    fn assert_a_problem(responses: &Value, status: &str) {
        assert!(
            responses[status]["content"]["application/problem+json"]["schema"]["$ref"]
                == "#/components/schemas/ProblemDetails",
            "{status} is documented as the problem it answers: {responses}",
        );
    }

    fn receipt_schema() -> Value {
        json!({ "$ref": "#/components/schemas/Receipt" })
    }

    /// A body whose first bytes arrived and whose rest never does.
    fn stalling() -> Body {
        Body::from_stream(
            futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"{"))])
                .chain(futures_util::stream::pending()),
        )
    }

    async fn stall(client: &TestClient, path: &str, media_type: &str) -> StatusCode {
        let resp = client
            .post(path)
            .content_type(media_type)
            .body(stalling())
            .send()
            .await;
        resp.0.status()
    }

    /// The edge refuses a declared length past the cap before routing, whatever
    /// the route reads: a route taking no body answers it as one taking one does.
    #[tokio::test]
    async fn a_route_taking_no_body_documents_the_413_a_declared_length_past_the_cap_earns() {
        let app = TestApp::for_module::<TightCapApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        for (method, path) in [("get", "/orders/plain"), ("delete", "/orders/nothing")] {
            let live = match method {
                "get" => app.http().get(path),
                _ => app.http().delete(path),
            }
            .header("content-length", "10")
            .body("0123456789")
            .send()
            .await;
            live.assert_status(StatusCode::PAYLOAD_TOO_LARGE);

            assert_a_problem(responses(&doc, path, method), "413");
        }
    }

    /// A problem document answered as a handler's success is an error whose
    /// status only the handler knows: OpenAPI's error ranges, with the problem
    /// it is, and no success.
    #[tokio::test]
    async fn a_problem_answered_bare_documents_the_error_ranges_it_answers_in() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for (path, status) in [
            ("/bodies/problem", StatusCode::NOT_FOUND),
            ("/bodies/problem/result", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let live = app.http().get(path).send().await;
            live.assert_status(status);
            live.assert_content_type("application/problem+json");

            let answers = responses(&doc, path, "get");
            assert!(
                answers
                    .as_object()
                    .is_some_and(|answers| answers.keys().all(|key| !key.starts_with('2'))),
                "a problem is no success: {answers}",
            );
            assert_a_problem(answers, "4XX");
            assert_a_problem(answers, "5XX");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_unguarded_route_documents_the_timeout_503() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let live = app.http().get("/orders/slow").send().await;
        live.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        live.assert_header_exist("retry-after");

        let answers = responses(&doc, "/orders/slow", "get");
        assert_a_problem(answers, "503");
        assert_eq!(
            answers["503"]["headers"]["Retry-After"]["schema"]["type"], "integer",
            "{answers}",
        );
    }

    #[tokio::test]
    async fn a_route_with_no_request_budget_documents_no_timeout_503() {
        let app = TestApp::for_module::<UnboundedApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        let answers = responses(&doc, "/orders/slow", "get");
        assert!(answers.get("503").is_none(), "{answers}");
    }

    #[tokio::test]
    async fn a_body_documents_the_415_its_media_type_is_refused_with() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for path in ["/orders/json", "/orders/form", "/orders/upload"] {
            let live = app
                .http()
                .post(path)
                .content_type("text/plain")
                .body("x")
                .send()
                .await;
            live.assert_status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            assert_a_problem(responses(&doc, path, "post"), "415");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_body_documents_the_408_a_stall_is_answered() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for (path, media_type) in [
            ("/orders/json", JSON_BODY),
            ("/orders/form", FORM_BODY),
            ("/orders/upload", MULTIPART_BODY),
            ("/orders/raw", "application/octet-stream"),
            ("/orders/stream", "application/octet-stream"),
        ] {
            assert_eq!(
                stall(app.http(), path, media_type).await,
                StatusCode::REQUEST_TIMEOUT,
                "{path}",
            );
            assert_a_problem(responses(&doc, path, "post"), "408");
        }
    }

    /// The budget counts from the arrival, so a gap past it never fires.
    #[tokio::test(start_paused = true)]
    async fn a_gap_past_the_request_budget_documents_no_408() {
        let app = TestApp::for_module::<GapPastTheBudgetApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        assert_eq!(
            stall(app.http(), "/orders/json", JSON_BODY).await,
            StatusCode::SERVICE_UNAVAILABLE,
        );
        let answers = responses(&doc, "/orders/json", "post");
        assert!(answers.get("408").is_none(), "{answers}");
        assert_a_problem(answers, "503");
    }

    #[tokio::test(start_paused = true)]
    async fn with_no_request_budget_the_gap_alone_answers_and_is_documented() {
        let app = TestApp::for_module::<UnboundedApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        assert_eq!(
            stall(app.http(), "/orders/json", JSON_BODY).await,
            StatusCode::REQUEST_TIMEOUT,
        );
        assert_a_problem(responses(&doc, "/orders/json", "post"), "408");
    }

    #[tokio::test]
    async fn a_raw_body_takes_any_media_type_and_documents_what_the_edge_refuses() {
        let app = TestApp::for_module::<TightCapApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for path in ["/orders/raw", "/orders/stream"] {
            let accepted = app
                .http()
                .post(path)
                .content_type("text/plain")
                .body("x")
                .send()
                .await;
            accepted.assert_status_is_ok();
            let refused = app.http().post(path).body(vec![0u8; 64]).send().await;
            refused.assert_status(StatusCode::PAYLOAD_TOO_LARGE);

            let body = &doc["paths"][path]["post"]["requestBody"];
            assert_eq!(body["required"], false, "{body}");
            assert_eq!(
                body["content"]["*/*"]["schema"],
                json!({ "type": "string", "format": "binary" }),
                "{body}",
            );
            let answers = responses(&doc, path, "post");
            assert_a_problem(answers, "413");
            assert!(answers.get("415").is_none(), "{answers}");
        }
    }

    #[tokio::test]
    async fn a_pair_documents_its_body_under_the_range_of_a_status_computed_at_run_time() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for path in ["/orders/pair", "/orders/result-pair"] {
            let live = app.http().post(path).send().await;
            live.assert_status(StatusCode::ACCEPTED);

            let answers = responses(&doc, path, "post");
            assert!(answers.get("200").is_none(), "{answers}");
            assert_eq!(
                answers["2XX"]["content"]["application/json"]["schema"],
                receipt_schema(),
                "{answers}",
            );
        }
        let text = responses(&doc, "/orders/text-pair", "post");
        assert!(text.get("200").is_none(), "{text}");
        assert_eq!(
            text["2XX"]["content"]["text/plain"]["schema"],
            json!({ "type": "string" }),
            "{text}",
        );
    }

    /// What a client reads off the response beside what the document says it
    /// reads: every spelling whose body no schema names, under the status it
    /// answers and the one the document keys it on.
    #[tokio::test]
    async fn an_answer_documents_the_media_type_it_writes() {
        const TEXT: &str = "text/plain";
        const OCTETS: &str = "application/octet-stream";
        let string = json!({ "type": "string" });
        let binary = json!({ "type": "string", "format": "binary" });

        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for (path, status, key, media_type, schema) in [
            ("/bodies/string", 200, "200", TEXT, &string),
            ("/bodies/str", 200, "200", TEXT, &string),
            ("/bodies/result", 200, "200", TEXT, &string),
            ("/bodies/bytes", 200, "200", OCTETS, &binary),
            ("/bodies/vec", 200, "200", OCTETS, &binary),
            ("/bodies/body", 200, "200", "*/*", &binary),
            ("/bodies/pair/string", 202, "2XX", TEXT, &string),
            ("/bodies/pair/str", 202, "2XX", TEXT, &string),
            ("/bodies/pair/bytes", 202, "2XX", OCTETS, &binary),
            ("/bodies/pair/vec", 202, "2XX", OCTETS, &binary),
            ("/bodies/pair/body", 202, "2XX", "*/*", &binary),
            ("/bodies/pair/result", 202, "2XX", OCTETS, &binary),
            ("/bodies/pair/created", 202, "2XX", OCTETS, &binary),
            ("/bodies/created", 201, "201", TEXT, &string),
            ("/bodies/paginated", 200, "200", OCTETS, &binary),
            ("/bodies/html", 200, "200", "text/html", &string),
            ("/bodies/declared", 200, "2XX", "text/csv", &string),
        ] {
            let live = app.http().get(path).send().await;
            assert_eq!(live.0.status(), status, "{path}");
            // `*/*` is a body that sets no type, which no client can read one off.
            let written = live
                .0
                .headers()
                .get(header::CONTENT_TYPE)
                .map_or("*/*", |written| {
                    written.to_str().expect("ascii").split(';').next().unwrap()
                });
            assert_eq!(written, media_type, "{path}");

            let answer = &responses(&doc, path, "get")[key];
            assert_eq!(
                answer["content"],
                json!({ media_type: { "schema": schema } }),
                "{path}: {answer}",
            );
        }
    }

    /// `#[response_header]` writes over what the type wrote, a schema included.
    #[tokio::test]
    async fn a_content_type_the_route_sets_keys_the_schema_its_type_names() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let live = app.http().get("/bodies/vendored").send().await;
        live.assert_header(header::CONTENT_TYPE, "application/vnd.api+json");
        assert_eq!(
            responses(&doc, "/bodies/vendored", "get")["200"]["content"],
            json!({ "application/vnd.api+json": { "schema": receipt_schema() } }),
        );
    }

    /// `Form` reads the query on `GET`, before any media type is looked at.
    #[tokio::test]
    async fn a_form_on_get_documents_its_query_and_nothing_of_a_body() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let read = app
            .http()
            .get("/orders/form-query?sku=x")
            .content_type("text/plain")
            .send()
            .await;
        read.assert_status_is_ok();
        let refused = app.http().get("/orders/form-query").send().await;
        refused.assert_status(StatusCode::BAD_REQUEST);

        let operation = &doc["paths"]["/orders/form-query"]["get"];
        assert!(operation.get("requestBody").is_none(), "{operation}");
        assert_eq!(
            operation["parameters"],
            json!([{
                "name": "sku",
                "in": "query",
                "required": true,
                "schema": { "type": "string" },
            }]),
        );
        let answers = &operation["responses"];
        assert_a_problem(answers, "400");
        for status in ["408", "415"] {
            assert!(answers.get(status).is_none(), "{status}: {answers}");
        }
        // The edge's, before routing, as on every route.
        assert_a_problem(answers, "413");
    }

    /// A map names no property to expand, and its values are still read.
    #[tokio::test]
    async fn a_parameter_map_documents_the_400_its_value_is_refused_with() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for (route, sent) in [
            ("/orders/free-query", "/orders/free-query?a=abc"),
            ("/orders/free-form-query", "/orders/free-form-query?a=abc"),
        ] {
            let refused = app.http().get(sent).send().await;
            refused.assert_status(StatusCode::BAD_REQUEST);
            assert_a_problem(responses(&doc, route, "get"), "400");
        }
    }

    #[tokio::test]
    async fn a_bare_status_documents_the_range_and_no_body() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        for path in ["/orders/status", "/orders/result-status"] {
            let live = app.http().delete(path).send().await;
            live.assert_status(StatusCode::NO_CONTENT);

            let answers = responses(&doc, path, "delete");
            assert!(answers.get("200").is_none(), "{answers}");
            assert!(answers["2XX"].get("content").is_none(), "{answers}");
            assert!(
                answers["2XX"]["description"]
                    .as_str()
                    .is_some_and(|said| said.contains("run time")),
                "{answers}",
            );
        }
    }

    #[tokio::test]
    async fn a_built_response_documents_the_range_and_the_body_it_declares() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        let live = app.http().post("/orders/built").send().await;
        live.assert_status(StatusCode::ACCEPTED);

        let answers = responses(&doc, "/orders/built", "post");
        assert!(answers.get("200").is_none(), "{answers}");
        assert_eq!(
            answers["2XX"]["content"]["application/json"]["schema"],
            receipt_schema(),
            "{answers}",
        );
    }

    /// An answer of the application's own: only its `into_response` knows the
    /// status.
    #[tokio::test]
    async fn an_answer_no_type_states_the_status_of_documents_the_range() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let own = app.http().post("/orders/own").send().await;
        own.assert_status(StatusCode::IM_A_TEAPOT);

        let answers = responses(&doc, "/orders/own", "post");
        assert!(answers.get("200").is_none(), "{answers}");
        assert!(answers.get("2XX").is_some(), "{answers}");
    }

    #[tokio::test]
    async fn http_code_pins_a_computed_status_and_the_document_names_it() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        let live = app.http().post("/orders/pinned-pair").send().await;
        live.assert_status(StatusCode::ACCEPTED);

        let answers = responses(&doc, "/orders/pinned-pair", "post");
        assert!(answers.get("2XX").is_none(), "{answers}");
        assert_eq!(
            answers["202"]["content"]["application/json"]["schema"],
            receipt_schema(),
            "{answers}",
        );
    }

    #[tokio::test]
    async fn an_answer_a_type_fixes_keeps_its_status() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;
        let answers = responses(&doc, "/orders/plain", "get");
        assert!(answers.get("2XX").is_none(), "{answers}");
        assert_eq!(
            answers["200"]["content"]["application/json"]["schema"],
            receipt_schema(),
            "{answers}",
        );
        let answers = responses(&doc, "/orders/nothing", "delete");
        assert!(answers.get("2XX").is_none(), "{answers}");
        assert!(answers.get("200").is_some(), "{answers}");
    }

    /// A path parameter is the router's to decode and the handler's type to
    /// read, so each can refuse the value a client sent.
    #[tokio::test]
    async fn a_path_parameter_documents_the_400_its_value_is_refused_with() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let undecodable = app.http().get("/orders/item/%FF").send().await;
        undecodable.assert_status(StatusCode::BAD_REQUEST);
        assert_a_problem(responses(&doc, "/orders/item/{name}", "get"), "400");

        let untyped = app.http().get("/orders/numbered/abc").send().await;
        untyped.assert_status(StatusCode::BAD_REQUEST);
        assert_a_problem(responses(&doc, "/orders/numbered/{id}", "get"), "400");
    }

    #[tokio::test]
    async fn an_optional_parameter_documents_the_400_its_value_is_refused_with() {
        let app = TestApp::for_module::<DefaultApp>().await.expect("boots");
        let doc = document(&app, "/api-json").await;

        let query = app.http().get("/orders/filtered?limit=abc").send().await;
        query.assert_status(StatusCode::BAD_REQUEST);
        assert_a_problem(responses(&doc, "/orders/filtered", "get"), "400");

        let header = app
            .http()
            .get("/orders/retried")
            .header("x-retried", "abc")
            .send()
            .await;
        header.assert_status(StatusCode::BAD_REQUEST);
        assert_a_problem(responses(&doc, "/orders/retried", "get"), "400");
    }

    #[tokio::test]
    async fn a_version_parameter_documents_the_400_and_the_404_it_is_answered() {
        let app = TestApp::for_module::<HeaderVersionedApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        let version = nest_rs_http::DEFAULT_VERSION_HEADER;

        let unserved = app.http().get("/posts").header(version, "9").send().await;
        unserved.assert_status(StatusCode::NOT_FOUND);
        let malformed = app
            .http()
            .get("/posts")
            .header(version, "not a token")
            .send()
            .await;
        malformed.assert_status(StatusCode::BAD_REQUEST);

        let answers = responses(&doc, "/posts", "get");
        assert_a_problem(answers, "400");
        assert_a_problem(answers, "404");
    }

    /// The selector refuses a malformed version in front of routing, on any
    /// address; a well-formed one moves no unversioned route, so no `404`.
    #[tokio::test]
    async fn an_unversioned_route_beside_a_versioned_one_documents_the_400_a_selector_answers() {
        let app = TestApp::for_module::<HeaderVersionedApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        let version = nest_rs_http::DEFAULT_VERSION_HEADER;

        let malformed = app
            .http()
            .get("/orders/plain")
            .header(version, "not a token")
            .send()
            .await;
        malformed.assert_status(StatusCode::BAD_REQUEST);
        let unserved = app
            .http()
            .get("/orders/plain")
            .header(version, "9")
            .send()
            .await;
        unserved.assert_status_is_ok();

        let answers = responses(&doc, "/orders/plain", "get");
        assert_a_problem(answers, "400");
        assert!(answers.get("404").is_none(), "{answers}");

        let app = TestApp::for_module::<MediaTypeVersionedApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        let malformed = app
            .http()
            .get("/orders/plain")
            .header(header::ACCEPT, "application/json; version=!!")
            .send()
            .await;
        malformed.assert_status(StatusCode::BAD_REQUEST);
        assert_a_problem(responses(&doc, "/orders/plain", "get"), "400");
    }

    /// With nothing versioned the selector is never mounted, so it refuses nothing.
    #[tokio::test]
    async fn a_strategy_with_nothing_versioned_documents_no_400() {
        let app = TestApp::for_module::<NothingVersionedApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        let read = app
            .http()
            .get("/orders/plain")
            .header(nest_rs_http::DEFAULT_VERSION_HEADER, "not a token")
            .send()
            .await;
        read.assert_status_is_ok();
        let answers = responses(&doc, "/orders/plain", "get");
        assert!(answers.get("400").is_none(), "{answers}");
    }

    #[tokio::test]
    async fn a_listed_origin_policy_documents_the_403_it_refuses_another_with() {
        let app = TestApp::for_module::<ListedOriginsApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        let refused = app
            .http()
            .get("/orders/plain")
            .header("origin", "https://elsewhere.example")
            .send()
            .await;
        refused.assert_status(StatusCode::FORBIDDEN);
        assert_a_problem(responses(&doc, "/orders/plain", "get"), "403");
    }

    #[tokio::test]
    async fn a_policy_admitting_every_origin_documents_no_403() {
        let app = TestApp::for_module::<EveryOriginApp>()
            .await
            .expect("boots");
        let doc = document(&app, "/api-json").await;
        let admitted = app
            .http()
            .get("/orders/plain")
            .header("origin", "https://elsewhere.example")
            .send()
            .await;
        admitted.assert_status_is_ok();
        let answers = responses(&doc, "/orders/plain", "get");
        assert!(answers.get("403").is_none(), "{answers}");
    }
}
