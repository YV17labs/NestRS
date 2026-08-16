//! URI versioning — and the layout the versioning page prescribes for it.
//!
//! > "Both controllers live in the feature's `http/controller.rs` — a version
//! > is a wire concern, not a second feature."
//!
//! Two versions of one route, in **one file**, with the **same handler name**,
//! is therefore *the* documented shape. It did not compile: `#[routes]` emits a
//! module-level type per handler (poem's `#[handler]` form), and the symbol was
//! derived from the method name alone — so `V1Controller::ping` and
//! `V2Controller::ping` collided in a namespace neither knew it shared. The
//! error named the mangled symbol and nothing connected it to "rename one of
//! your handlers". Nothing about it was versioning-specific either: `list` /
//! `get` / `create` are exactly the names two controllers in one file repeat.

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

// Same handler name, same file — only the `Json<T>` shape differs in the real
// pattern the page describes, so the method names are expected to match.
#[controller(path = "/fund", version = "2")]
struct FundV2Controller;

#[routes]
impl FundV2Controller {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "v2".into()
    }
}

// A third controller with no version and the same handler name again: the
// collision is about sharing a file, not about versioning.
#[controller(path = "/unversioned")]
struct UnversionedController;

#[routes]
impl UnversionedController {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "none".into()
    }
}

// The shape a second API version actually has: most routes unchanged, one
// added. `version = ["1", "2"]` mounts the whole controller twice, and the one
// route that is new to v2 says so itself — no duplicate struct, no duplicate
// handler bodies.
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

    // The path exists in v1 — its `GET` mounts there — so the honest answer for
    // the verb that does not is `405`, not `404`.
    client
        .post("/v1/reports")
        .send()
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn a_narrowed_route_narrows_under_every_selection_strategy() {
    // The version moves out of the path and into a header; which routes exist
    // per version must not move with it.
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

    // The unversioned form of a versioned controller is not mounted.
    client
        .get("/fund/ping")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// Boot the same three controllers under a selection strategy (`None` leaves
/// the default URI one) and, optionally, a global prefix. Nothing about the
/// controllers changes — that is the point: a version is declared once, and
/// both how a caller *selects* one and what path the deployment hands them are
/// decided out here.
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

    // An unversioned controller is reached by stating no version at all.
    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;

    // And the URI form is no longer a second address for the same operation.
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

    // No `Accept` version: the configured default answers, so an old client
    // keeps working when a `v2` ships.
    let resp = client.get("/fund/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v1").await;
}

#[tokio::test]
async fn an_unknown_version_is_a_404_not_a_fallback() {
    // Serving *some* version to a caller that asked for one we do not have is
    // how a client silently talks to the wrong API.
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
/// no version, so nothing may rewrite it. The bug this pins was real and
/// silent: a deployment that set `NESTRS_HTTP__DEFAULT_VERSION` sent *every*
/// unversioned path to `/v{default}/…`, taking the GraphQL endpoint, the MCP
/// endpoint, the OpenAPI document and the health probes down with it.
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

    // The versioned path takes the default.
    let resp = client.get("/fund/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v1").await;

    // The unversioned controller is served as written — the default is a
    // default *among versions*, not a rewrite of everything the app mounts.
    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;
}

/// The prefix a deployment mounts the app under, as `HttpConfig.global_prefix`
/// spells it — a reverse proxy hands off `/api`, and every path below is
/// written the way a client behind that proxy writes it.
const PREFIX: &str = "/api";

/// The rewrite is placed **inside** the global prefix on purpose, so the path
/// it rewrites is the one controllers mount at. That makes the two compose in
/// one direction only, and both halves of it need pinning: the caller addresses
/// the prefix and states the version out of band, and the router serves
/// `<prefix>/v<n>/…`. Read the other way round — a version segment outside the
/// prefix, or a prefix the rewrite has to re-add — nothing is reachable.
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

    // The documented call: the prefix in the path, the version in the header.
    for (version, body) in [("1", "v1"), ("2", "v2")] {
        let resp = client
            .get(format!("{PREFIX}/fund/ping"))
            .header(DEFAULT_VERSION_HEADER, version)
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }

    // An unversioned controller is served under the prefix and nothing else —
    // the rewrite leaves it alone, the prefix does not.
    let resp = client
        .get(format!("{PREFIX}/unversioned/ping"))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;

    // Neither half is optional, and neither becomes a second address.
    for path in [
        // The prefix is not bypassable by stating a version.
        "/fund/ping",
        // The URI form under a header strategy — refused inside the prefix
        // exactly as it is refused without one.
        "/api/v1/fund/ping",
        // The version read *outside* the prefix: the mount is `/api`, so this
        // never reaches the rewrite at all.
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

/// The same composition under the default strategy, where the version *is* the
/// path: the prefix wraps the mounted `/v<n>/…`, so a client writes both.
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

    // The version segment belongs under the prefix, not in front of it.
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
    // The sharp case, and the one that decides where the line sits:
    // `/fund/ping` HAS versions, so answering v9 with v1's body would be the
    // silent fallback this strategy exists to prevent.
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
    // The other side of the same line. A client sets the version header once,
    // globally — that is the whole appeal of header versioning — and then hits
    // an unversioned controller. `/unversioned/ping` has exactly one shape, so
    // there is no other version the caller could have meant and nothing to be
    // silent about. Answering `404` here made every unversioned controller
    // unreachable from a correctly-configured client.
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
    // The same neutrality, arrived at from the deployment side rather than the
    // client's: `NESTRS_HTTP__DEFAULT_VERSION` is a default *among versions*.
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

    // …while it still answers for a path that does have versions.
    let resp = client.get("/fund/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("v2").await;
}

/// `HttpTransport::api_versioning` accepts any selector, and a URI one is a
/// no-op by definition — routing already resolves it. Wrapping it anyway made
/// every `/v{n}/…` path a `404`, because the wrapper refuses the URI form.
/// Unreachable through `HttpModule` (which hands over `None` for `uri`), which
/// is exactly why it needs a test at the builder.
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

// A versioned controller at the **root** path. `/` matched nothing segment-wise,
// so this whole controller was silently version-neutral: a caller asking for v2
// got v1's body, and an unknown version answered `200`.
#[controller(path = "/", version = "2")]
struct RootV2Controller;

#[routes]
impl RootV2Controller {
    #[get("/root-ping")]
    async fn ping(&self) -> String {
        "root-v2".into()
    }
}

// A separate, unversioned controller nested under a versioned one's prefix.
// Prefix matching dragged it into the versioned namespace, so a deployment-wide
// default version made it unreachable with no client involvement at all.
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

    // And an unknown version is still refused rather than silently answered.
    client
        .get("/root-ping")
        .header(DEFAULT_VERSION_HEADER, "9")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_root_versioned_controller_does_not_swallow_the_self_mounted_paths() {
    // The root controller declares a version, but it declares *routes*, not the
    // whole namespace: nothing versioned answers at `/unversioned/ping`, so it
    // stays neutral even with a deployment-wide default.
    let client = boot_edge_cases(Some("2")).await;
    let resp = client.get("/unversioned/ping").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;
}

#[tokio::test]
async fn a_controller_nested_under_a_versioned_prefix_stays_reachable() {
    let client = boot_edge_cases(Some("1")).await;

    // No client involvement at all — the deployment names a default, and this
    // controller has no versions, so it is served as written.
    let resp = client.get("/fund/drafts").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("drafts").await;

    // Stating a version does not change that.
    let resp = client
        .get("/fund/drafts")
        .header(DEFAULT_VERSION_HEADER, "1")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("drafts").await;

    // While its versioned neighbour still resolves normally.
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
    // The paths are identical *by design* — that is the documented
    // two-controller layout — so "give each one a distinct path" was advice
    // against the docs, about a string (`/v2/overlap`) neither developer wrote.
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

// A segment that mixes a literal with a parameter — how a handle or a slug is
// written. Compared as a literal it never matched, so the address was declared
// unversioned and a caller asking for v2 was served the *other* controller's
// body, in silence.
#[controller(path = "/mix", version = "2")]
struct MixV2Controller;

#[routes]
impl MixV2Controller {
    #[get("/@:handle")]
    async fn handle(&self) -> String {
        "mix-v2".into()
    }
}

#[controller(path = "/mix")]
struct MixNeutralController;

#[routes]
impl MixNeutralController {
    #[get("/@:handle")]
    async fn handle(&self) -> String {
        "mix-neutral".into()
    }
}

// A versioned catch-all at the root. With a default version it rewrote every
// address in the app — including the self-mounted endpoints and every
// unversioned controller — which is the failure this module was built around.
#[controller(path = "/", version = "1")]
struct RootCatchAllController;

#[routes]
impl RootCatchAllController {
    #[get("/*rest")]
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

    // Stating nothing keeps the unversioned neighbour.
    let resp = client.get("/mix/@bob").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("mix-neutral").await;
}

#[tokio::test]
async fn a_default_version_does_not_let_a_root_catch_all_swallow_the_app() {
    let client = boot_route_shapes(Some("1")).await;

    // A real unversioned controller keeps answering.
    let resp = client.get("/live/probe").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("live").await;

    // And the catch-all still answers where nothing else does.
    let resp = client.get("/anything/else").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("root-catchall").await;
}

/// A version header carrying a byte that is legal on the wire but is not text.
///
/// `HEADER_VALUE_MAP` admits `0x80..=0xFF`, so hyper delivers the header and
/// `HeaderValue::to_str` refuses it. That refusal used to be `.ok()`-ed into
/// `None` — "the caller stated nothing" — which then served the deployment
/// default, or the unversioned controller at the same address, at `200`. One
/// byte decided which controller answered, and the docs and the CHANGELOG both
/// promise a `400`.
#[tokio::test]
async fn a_version_header_that_is_not_text_is_refused_rather_than_read_as_silence() {
    let client = boot_edge_cases(Some("2")).await;

    // The control: a well-formed header at the same address.
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

/// The same hole on the media-type strategy, which decoded `Accept` with the
/// same discarded error.
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

    // An `Accept` with no `version=` parameter at all is still an ordinary
    // caller who stated nothing, not a malformed statement.
    let resp = client
        .get("/unversioned/ping")
        .header(header::ACCEPT, "application/json")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("none").await;
}

/// A version header that *is* text and is still not a version.
///
/// The undecodable case above is refused for being unreadable; this one is
/// readable and refused for its shape — a path traversal, a URL, a sentence.
/// Both answer `400`, which is exactly why the event matters: from the outside
/// the two are one status code, and only the log separates "a client sent
/// binary junk" from "a client is probing the version selector".
///
/// The event deliberately carries the **length** and not the value: a rejected
/// version is attacker-controlled text, and logging it verbatim would let a
/// caller write into the operator's log.
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
