//! API versioning, over the documented layout: two versions of one route in
//! one file, with the same handler name.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{
    ApiVersioning, DEFAULT_VERSION_HEADER, HttpTransport, VersionSelector, controller, routes,
};
use poem::Response;
use poem::endpoint::BoxEndpoint;
use poem::http::{HeaderName, StatusCode, header};
use poem::test::TestClient;

#[controller(path = "/fund", version = "1")]
struct FundV1Controller;

#[routes]
impl FundV1Controller {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "v1".into()
    }
}

// Same handler name, same file: the documented layout.
#[controller(path = "/fund", version = "2")]
struct FundV2Controller;

#[routes]
impl FundV2Controller {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "v2".into()
    }
}

#[controller(path = "/unversioned")]
struct UnversionedController;

#[routes]
impl UnversionedController {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "none".into()
    }
}

// Mounted under both versions; the route new to v2 narrows itself.
#[controller(path = "/reports", version = ["1", "2"])]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "list".into()
    }

    #[post("/")]
    #[version("2")]
    async fn create(&self) -> String {
        "created".into()
    }
}

#[module(providers = [
    FundV1Controller,
    FundV2Controller,
    UnversionedController,
    ReportsController,
])]
struct VersionedModule;

#[tokio::test]
async fn one_controller_mounts_under_every_version_it_declares() {
    let client = boot_with(None, None).await;

    for path in ["/v1/reports", "/v2/reports"] {
        let resp = client.get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_text("list").await;
    }
}

#[tokio::test]
async fn a_route_narrowed_to_one_version_is_absent_from_the_others() {
    let client = boot_with(None, None).await;

    let resp = client.post("/v2/reports").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("created").await;

    // The path exists in v1, so the verb it lacks is a `405`, not a `404`.
    client
        .post("/v1/reports")
        .send()
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn a_narrowed_route_narrows_under_every_selection_strategy() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        None,
    )
    .await;

    let resp = client
        .post("/reports")
        .header(DEFAULT_VERSION_HEADER, "2")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("created").await;

    client
        .post("/reports")
        .header(DEFAULT_VERSION_HEADER, "1")
        .send()
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);

    let resp = client
        .get("/reports")
        .header(DEFAULT_VERSION_HEADER, "1")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("list").await;
}

#[tokio::test]
async fn two_controllers_in_one_file_may_share_a_handler_name() {
    let client = boot_with(None, None).await;

    for (path, body) in [
        ("/v1/fund/ping", "v1"),
        ("/v2/fund/ping", "v2"),
        ("/unversioned/ping", "none"),
    ] {
        let resp = client.get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }

    client
        .get("/fund/ping")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// Boot the same three controllers under a selection strategy (`None` leaves
/// the default URI one) and, optionally, a global prefix.
async fn boot_with(
    selector: impl Into<Option<VersionSelector>>,
    prefix: Option<&str>,
) -> TestClient<BoxEndpoint<'static, Response>> {
    let app = App::builder()
        .module::<VersionedModule>()
        .build()
        .await
        .expect("boots");
    let mut transport = HttpTransport::new();
    if let Some(selector) = selector.into() {
        transport = transport.api_versioning(selector);
    }
    if let Some(prefix) = prefix {
        transport = transport.global_prefix(prefix);
    }
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    TestClient::new(
        transport
            .take_endpoint()
            .expect("configure populates the endpoint"),
    )
}

#[tokio::test]
async fn a_header_selects_the_version_of_the_same_controllers() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        None,
    )
    .await;

    for (version, body) in [("1", "v1"), ("2", "v2")] {
        let resp = client
            .get("/fund/ping")
            .header(DEFAULT_VERSION_HEADER, version)
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }

    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;

    client
        .get("/v1/fund/ping")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_media_type_selects_the_version_and_a_default_covers_a_silent_caller() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::MediaType,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            Some("1".into()),
        ),
        None,
    )
    .await;

    let resp = client
        .get("/fund/ping")
        .header(header::ACCEPT, "application/json; version=2")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("v2").await;

    let resp = client.get("/fund/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v1").await;
}

#[tokio::test]
async fn an_unknown_version_is_a_404_not_a_fallback() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        None,
    )
    .await;
    client
        .get("/fund/ping")
        .header(DEFAULT_VERSION_HEADER, "9")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// A self-mounted endpoint — `/graphql`, `/mcp`, `/api-json`, `/health` — has
/// no version, so a default version must not rewrite it.
#[tokio::test]
async fn a_default_version_does_not_rewrite_paths_that_have_no_version() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            Some("1".into()),
        ),
        None,
    )
    .await;

    let resp = client.get("/fund/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v1").await;

    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;
}

/// The prefix a deployment mounts the app under (`HttpConfig.global_prefix`).
const PREFIX: &str = "/api";

#[tokio::test]
async fn a_version_resolves_inside_the_global_prefix_and_only_inside_it() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        Some(PREFIX),
    )
    .await;

    for (version, body) in [("1", "v1"), ("2", "v2")] {
        let resp = client
            .get(format!("{PREFIX}/fund/ping"))
            .header(DEFAULT_VERSION_HEADER, version)
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }

    let resp = client
        .get(format!("{PREFIX}/unversioned/ping"))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;

    for path in [
        "/fund/ping",
        "/api/v1/fund/ping",
        // The mount is `/api`, so this never reaches the rewrite.
        "/v1/api/fund/ping",
    ] {
        let resp = client
            .get(path)
            .header(DEFAULT_VERSION_HEADER, "1")
            .send()
            .await;
        assert_eq!(
            resp.0.status(),
            StatusCode::NOT_FOUND,
            "{path} must not be a second address for the versioned route",
        );
    }
}

#[tokio::test]
async fn the_uri_strategy_mounts_its_versions_under_the_global_prefix_too() {
    let client = boot_with(None, Some(PREFIX)).await;

    for (path, body) in [
        ("/api/v1/fund/ping", "v1"),
        ("/api/v2/fund/ping", "v2"),
        ("/api/unversioned/ping", "none"),
    ] {
        let resp = client.get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }

    client
        .get("/v1/api/fund/ping")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    client
        .get("/v1/fund/ping")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_stated_version_a_path_does_not_serve_never_falls_through() {
    // `/fund/ping` has versions, so v9 must not be served v1's body.
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        None,
    )
    .await;
    client
        .get("/fund/ping")
        .header(DEFAULT_VERSION_HEADER, "9")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_path_with_no_versions_is_served_whatever_version_is_stated() {
    // A client sets the version header globally; `/unversioned/ping` has one
    // shape, so it still answers.
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        None,
    )
    .await;
    for stated in ["1", "2", "9"] {
        let resp = client
            .get("/unversioned/ping")
            .header(DEFAULT_VERSION_HEADER, stated)
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("none").await;
    }
}

#[tokio::test]
async fn a_default_version_never_reaches_a_path_that_has_none() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            Some("2".into()),
        ),
        None,
    )
    .await;
    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;

    let resp = client.get("/fund/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v2").await;
}

/// A URI selector handed to the builder must not wrap the routes, whose
/// rewrite refuses the URI form; `HttpModule` never hands one over.
#[tokio::test]
async fn the_uri_strategy_passed_to_the_builder_is_a_no_op() {
    let client = boot_with(
        VersionSelector::new(
            ApiVersioning::Uri,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            None,
        ),
        None,
    )
    .await;

    for (path, body) in [
        ("/v1/fund/ping", "v1"),
        ("/v2/fund/ping", "v2"),
        ("/unversioned/ping", "none"),
    ] {
        let resp = client.get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }
}

#[controller(path = "/", version = "2")]
struct RootV2Controller;

#[routes]
impl RootV2Controller {
    #[get("/root-ping")]
    async fn ping(&self) -> String {
        "root-v2".into()
    }
}

#[controller(path = "/fund/drafts")]
struct FundDraftsController;

#[routes]
impl FundDraftsController {
    #[get("/")]
    async fn list(&self) -> String {
        "drafts".into()
    }
}

#[module(providers = [
    FundV1Controller,
    FundV2Controller,
    UnversionedController,
    ReportsController,
    RootV2Controller,
    FundDraftsController,
])]
struct EdgeCaseModule;

async fn boot_edge_cases(default: Option<&str>) -> TestClient<BoxEndpoint<'static, Response>> {
    let app = App::builder()
        .module::<EdgeCaseModule>()
        .build()
        .await
        .expect("boots");
    let mut transport = HttpTransport::new();
    transport = transport.api_versioning(VersionSelector::new(
        ApiVersioning::Header,
        HeaderName::from_static(DEFAULT_VERSION_HEADER),
        default.map(str::to_owned),
    ));
    transport
        .configure(app.container())
        .await
        .expect("configures");
    TestClient::new(transport.take_endpoint().expect("an endpoint"))
}

#[tokio::test]
async fn a_versioned_controller_at_the_root_path_still_selects() {
    let client = boot_edge_cases(None).await;

    let resp = client
        .get("/root-ping")
        .header(DEFAULT_VERSION_HEADER, "2")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("root-v2").await;

    client
        .get("/root-ping")
        .header(DEFAULT_VERSION_HEADER, "9")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_root_versioned_controller_does_not_swallow_the_self_mounted_paths() {
    // A versioned root declares routes, not the namespace below it.
    let client = boot_edge_cases(Some("2")).await;
    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;
}

#[tokio::test]
async fn a_controller_nested_under_a_versioned_prefix_stays_reachable() {
    let client = boot_edge_cases(Some("1")).await;

    let resp = client.get("/fund/drafts").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("drafts").await;

    let resp = client
        .get("/fund/drafts")
        .header(DEFAULT_VERSION_HEADER, "1")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("drafts").await;

    let resp = client
        .get("/fund/ping")
        .header(DEFAULT_VERSION_HEADER, "2")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("v2").await;
}

#[controller(path = "/overlap", version = ["1", "2"])]
struct OverlapAController;

#[routes]
impl OverlapAController {
    #[get("/")]
    async fn list(&self) -> String {
        "a".into()
    }
}

#[controller(path = "/overlap", version = ["2", "3"])]
struct OverlapBController;

#[routes]
impl OverlapBController {
    #[get("/")]
    async fn list(&self) -> String {
        "b".into()
    }
}

#[module(providers = [OverlapAController, OverlapBController])]
struct OverlappingModule;

#[tokio::test]
async fn two_controllers_overlapping_on_one_version_are_told_which_version() {
    let app = App::builder()
        .module::<OverlappingModule>()
        .build()
        .await
        .expect("boots");
    let err = HttpTransport::new()
        .configure(app.container())
        .await
        .expect_err("two controllers claiming /v2/overlap is a boot failure")
        .to_string();
    assert!(
        err.contains("OverlapAController") && err.contains("OverlapBController"),
        "the failure names both claimants: {err}",
    );
    assert!(
        err.contains(r#"version "2""#),
        "and the version they actually collide on: {err}",
    );
    assert!(
        err.contains("#[controller(version"),
        "and the list to edit: {err}",
    );
}

// A segment mixing a literal with a parameter, as a handle or a slug is written.
#[controller(path = "/mix", version = "2")]
struct MixV2Controller;

#[routes]
impl MixV2Controller {
    #[get("/@{handle}")]
    async fn handle(&self) -> String {
        "mix-v2".into()
    }
}

#[controller(path = "/mix")]
struct MixNeutralController;

#[routes]
impl MixNeutralController {
    #[get("/@{handle}")]
    async fn handle(&self) -> String {
        "mix-neutral".into()
    }
}

#[controller(path = "/", version = "1")]
struct RootCatchAllController;

#[routes]
impl RootCatchAllController {
    #[get("/{*rest}")]
    async fn any(&self) -> String {
        "root-catchall".into()
    }
}

#[controller(path = "/live")]
struct LiveController;

#[routes]
impl LiveController {
    #[get("/probe")]
    async fn probe(&self) -> String {
        "live".into()
    }
}

#[module(providers = [
    MixV2Controller,
    MixNeutralController,
    RootCatchAllController,
    LiveController,
])]
struct RouteShapeModule;

async fn boot_route_shapes(default: Option<&str>) -> TestClient<BoxEndpoint<'static, Response>> {
    let app = App::builder()
        .module::<RouteShapeModule>()
        .build()
        .await
        .expect("boots");
    let mut transport = HttpTransport::new();
    transport = transport.api_versioning(VersionSelector::new(
        ApiVersioning::Header,
        HeaderName::from_static(DEFAULT_VERSION_HEADER),
        default.map(str::to_owned),
    ));
    transport
        .configure(app.container())
        .await
        .expect("configures");
    TestClient::new(transport.take_endpoint().expect("an endpoint"))
}

#[tokio::test]
async fn a_literal_and_a_parameter_in_one_segment_still_select() {
    let client = boot_route_shapes(None).await;

    let resp = client
        .get("/mix/@bob")
        .header(DEFAULT_VERSION_HEADER, "2")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("mix-v2").await;

    let resp = client.get("/mix/@bob").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("mix-neutral").await;
}

#[tokio::test]
async fn a_default_version_does_not_let_a_root_catch_all_swallow_the_app() {
    let client = boot_route_shapes(Some("1")).await;

    let resp = client.get("/live/probe").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("live").await;

    let resp = client.get("/anything/else").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("root-catchall").await;
}

/// A version header carrying a byte that is legal on the wire but is not text
/// (`0x80..=0xFF`, which `HeaderValue::to_str` refuses).
#[tokio::test]
async fn a_version_header_that_is_not_text_is_refused_rather_than_read_as_silence() {
    let client = boot_edge_cases(Some("2")).await;

    let resp = client
        .get("/unversioned/ping")
        .header(DEFAULT_VERSION_HEADER, "2")
        .send()
        .await;
    resp.assert_status_is_ok();

    let undecodable =
        poem::http::HeaderValue::from_bytes(&[b'2', 0xff]).expect("a legal header value");
    client
        .get("/unversioned/ping")
        .header(DEFAULT_VERSION_HEADER, undecodable)
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_accept_header_that_is_not_text_is_refused_too() {
    let app = App::builder()
        .module::<EdgeCaseModule>()
        .build()
        .await
        .expect("boots");
    let mut transport = HttpTransport::new().api_versioning(VersionSelector::new(
        ApiVersioning::MediaType,
        HeaderName::from_static(DEFAULT_VERSION_HEADER),
        Some("2".to_owned()),
    ));
    transport
        .configure(app.container())
        .await
        .expect("configures");
    let client = TestClient::new(transport.take_endpoint().expect("an endpoint"));

    let undecodable = poem::http::HeaderValue::from_bytes(b"application/json; version=2\xff")
        .expect("a legal header value");
    client
        .get("/unversioned/ping")
        .header(header::ACCEPT, undecodable)
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let resp = client
        .get("/unversioned/ping")
        .header(header::ACCEPT, "application/json")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;
}

/// A version header that *is* text and is still not a version. The event
/// carries its length, never the attacker-controlled value.
#[tokio::test]
async fn a_version_header_that_is_text_but_malformed_is_rejected_and_reported() {
    let logs = nest_rs_testing::LogCapture::install();
    let client = boot_edge_cases(Some("2")).await;

    let resp = client
        .get("/unversioned/ping")
        .header(DEFAULT_VERSION_HEADER, "../../etc/passwd")
        .send()
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let event = logs.expect_one("nest_rs::http", "rejected a malformed API version");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("length").as_deref(), Some("16"));
    assert!(
        event.field("strategy").is_some(),
        "the event names which selector rejected it, got {:?}",
        event.fields,
    );
    assert!(
        !event.fields.values().any(|v| v.contains("passwd")),
        "the rejected value is attacker-controlled and must not reach the log: {:?}",
        event.fields,
    );
}
