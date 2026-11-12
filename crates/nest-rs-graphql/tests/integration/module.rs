//! Covers `src/module.rs` — a setup imported twice mounts `/graphql` once,
//! the module attaches the HTTP transport it mounts on, a listed resolver
//! brings it, even beside the transport alone, and a container no app sealed
//! mounts no schema.

use nest_rs_core::{
    App, AppLifecycle, Collecting, ContainerBuilder, Discoverable, Discovery, Module, Registering,
    module, target,
};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{HttpConfig, HttpEndpointMeta, HttpModule};
use nest_rs_testing::{HeadlessApp, LogCapture, TestApp};

use crate::in_profile;

#[resolver]
struct PingResolver;

#[operations]
impl PingResolver {
    #[query]
    #[public]
    async fn ping(&self) -> nest_rs_graphql::async_graphql::Result<String> {
        Ok("pong".into())
    }
}

/// A feature configuring the module as the app does.
#[module(imports = [GraphqlModule], providers = [PingResolver])]
struct ConfiguringPingModule;

/// A feature declaring the dependency bare.
#[module(imports = [GraphqlModule], providers = [PingResolver])]
struct BarePingModule;

#[module(imports = [GraphqlModule, ConfiguringPingModule])]
struct TwiceConfiguredApp;

#[module(imports = [BarePingModule, GraphqlModule])]
struct BareBesideTheConfiguredApp;

/// `/graphql` mounts once and answers, imported by `M`.
async fn mounts_once<M: nest_rs_core::Module>() {
    let app = TestApp::for_module::<M>()
        .await
        .unwrap_or_else(|refused| panic!("one endpoint for both imports: {refused:#}"));

    assert_eq!(
        Discovery::new(app.container())
            .meta::<HttpEndpointMeta>()
            .len(),
        1
    );
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_json(serde_json::json!({ "data": { "ping": "pong" } }))
        .await;
}

#[tokio::test]
async fn two_imports_mount_the_endpoint_once() {
    mounts_once::<TwiceConfiguredApp>().await;
}

#[tokio::test]
async fn a_bare_import_beside_the_configured_one_mounts_the_endpoint_once() {
    mounts_once::<BareBesideTheConfiguredApp>().await;
}

#[tokio::test]
async fn the_module_attaches_the_http_transport_it_mounts_on() {
    let app = HeadlessApp::builder()
        .module::<ConfiguringPingModule>()
        .build_headless()
        .await
        .expect("the module boots");

    assert_eq!(app.attached_transports(), ["http"]);
}

/// An adapter as a developer writes it: the resolver, and nothing of the edge's.
#[module(providers = [PingResolver])]
struct PingGraphqlModule;

/// The transport pinned, and the resolver's adapter beside it.
#[module(imports = [HttpModule::for_root(HttpConfig::default()), PingGraphqlModule])]
struct TransportBesideApp;

#[tokio::test]
async fn a_listed_resolver_brings_the_endpoint_and_the_transport_it_mounts_on() {
    mounts_once::<PingGraphqlModule>().await;
    let app = HeadlessApp::builder()
        .module::<PingGraphqlModule>()
        .build_headless()
        .await
        .expect("the module boots");

    assert_eq!(app.attached_transports(), ["http"]);
}

#[tokio::test]
async fn a_listed_resolver_brings_the_endpoint_beside_the_transport_alone() {
    mounts_once::<TransportBesideApp>().await;
}

/// A mount has no `Result`: the schema's discovery files its refusal on the
/// container, and the boot fails on it once its transports are configured.
#[tokio::test]
async fn the_schema_mounted_over_a_container_no_app_sealed_is_refused_naming_its_reader() {
    // Built by hand, so no wiring step refuses first.
    let container = nest_rs_core::Container::builder()
        .provide(nest_rs_graphql::GraphqlConfig::default())
        .provide(nest_rs_http::HttpConfig::default())
        .import::<BarePingModule>()
        .build()
        .expect("the container builds");
    let mut transport =
        nest_rs_http::HttpTransport::from_config(&nest_rs_http::HttpConfig::default())
            .expect("the defaults are in range");
    nest_rs_core::Transport::configure(&mut transport, &container)
        .await
        .expect("the mount files its refusal rather than returning it");

    let Some(refused) = nest_rs_core::__private::take_site_refusal(&container) else {
        panic!("the schema would serve every resolver linked into the binary");
    };
    let ungated = refused
        .downcast_ref::<nest_rs_core::UngatedDiscoveryError>()
        .unwrap_or_else(|| panic!("not the ungated refusal: {refused:#}"));
    assert_eq!(ungated.reader, "GraphQL resolver");
}

/// Both surfaces pinned open, as a deployment that wants them writes it.
#[module(
    imports = [GraphqlModule::for_root(nest_rs_graphql::GraphqlConfig {
        playground: nest_rs_config::Surface::Open,
        introspection: nest_rs_config::Surface::Open,
        ..nest_rs_graphql::GraphqlConfig::default()
    })],
    providers = [PingResolver],
)]
struct OpenedApp;

/// A pin opening the playground and introspection outside a dev profile is
/// honoured, and the boot mounting them says so once for each.
#[test]
fn a_pin_opening_the_surfaces_in_production_is_said_at_boot() {
    let logs = in_profile("production", || async {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<OpenedApp>()
            .await
            .unwrap_or_else(|refused| panic!("an explicit opening is honoured: {refused:#}"));
        app.http()
            .get("/graphql")
            .send()
            .await
            .assert_status_is_ok();
        logs
    });
    for surface in [
        "GraphQL playground is enabled and public outside a dev profile",
        "GraphQL introspection is enabled and public outside a dev profile",
    ] {
        let event = logs.expect_one(nest_rs_graphql::TARGET, surface);
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("environment").as_deref(), Some("production"));
    }
}

/// Where a pin of the path alone writes the SDL, per process.
fn emitted_sdl() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "nest_rs_graphql_by_profile_{}.graphql",
        std::process::id()
    ))
}

/// One field pinned: the emit stays the profile's.
#[module(
    imports = [GraphqlModule::for_root(nest_rs_graphql::GraphqlConfig {
        emit_path: emitted_sdl(),
        ..nest_rs_graphql::GraphqlConfig::default()
    })],
    providers = [PingResolver],
)]
struct EmitPathPinnedApp;

/// A pin of another field closes nothing it does not name and opens nothing in
/// a deployment, a process declaring no profile among them, which boots silent.
/// Under test, parallel suites would rewrite a committed file.
#[test]
fn a_pin_over_the_default_leaves_every_surface_to_the_profile() {
    for (profile, open, writes) in [
        ("development", true, true),
        ("test", true, false),
        ("staging", false, false),
        ("production", false, false),
        ("", false, false),
    ] {
        let _ = std::fs::remove_file(emitted_sdl());
        let (page, introspected) = in_profile(profile, || async {
            let logs = nest_rs_testing::LogCapture::install();
            let app = TestApp::for_module::<EmitPathPinnedApp>()
                .await
                .unwrap_or_else(|refused| panic!("{profile:?} boots: {refused:#}"));
            for surface in [
                "GraphQL playground is enabled and public outside a dev profile",
                "GraphQL introspection is enabled and public outside a dev profile",
            ] {
                logs.expect_none(nest_rs_graphql::TARGET, surface);
            }
            let page = app.http().get("/graphql").send().await.0.status();
            let body: serde_json::Value = app
                .http()
                .post("/graphql")
                .body_json(&serde_json::json!({ "query": "{ __schema { queryType { name } } }" }))
                .send()
                .await
                .json()
                .await
                .value()
                .deserialize();
            (page, body["data"]["__schema"].is_object())
        });
        assert_eq!(page.is_success(), open, "{profile:?}: the playground");
        assert_eq!(introspected, open, "{profile:?}: introspection");
        assert_eq!(emitted_sdl().exists(), writes, "{profile:?}: the emit");
    }
    let _ = std::fs::remove_file(emitted_sdl());
}

/// What `App::run` files for a host whose edge nothing in the boot imports.
const UNSERVED: &str = "host not served: its edge's transport is not attached";

/// Registered as `#[module]` registers a provider it lists, with nothing of the
/// edge's beside it.
struct HandRegistersPing;

impl Module for HandRegistersPing {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        <PingResolver as Discoverable>::register(builder)
    }
}

#[tokio::test]
async fn a_resolver_registered_by_hand_with_no_transport_is_named_at_run() {
    let logs = LogCapture::install();
    App::builder()
        .module::<HandRegistersPing>()
        .build()
        .await
        .expect("the app boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.field("host").as_deref(), Some("PingResolver"));
    assert_eq!(unserved.field("import").as_deref(), Some("GraphqlModule"));
}

#[tokio::test]
async fn a_resolver_seeded_with_no_transport_is_named_at_run() {
    let logs = LogCapture::install();
    App::builder()
        .provide(PingResolver)
        .build()
        .await
        .expect("the app boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.field("import").as_deref(), Some("GraphqlModule"));
}

/// Registers the host itself beside its module, which mounts what a `#[module]`
/// lists: what the line above says to import is here, and the edge's own line
/// takes over.
struct HandRegistersPingBesideGraphql;

impl Module for HandRegistersPingBesideGraphql {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<GraphqlModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        <PingResolver as Discoverable>::register(builder.import::<GraphqlModule>())
    }
}

#[module(imports = [
    HttpModule::for_root(HttpConfig {
        host: "127.0.0.1".into(),
        port: 0,
        ..HttpConfig::default()
    }),
    HandRegistersPingBesideGraphql,
])]
struct HandRegisteredPingOnTheLoopback;

#[tokio::test]
async fn a_resolver_registered_by_hand_beside_its_module_is_told_to_list_it() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<HandRegistersPingBesideGraphql>()
        .build()
        .await
        .expect("the module boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    let body = resp.0.into_body().into_string().await.unwrap_or_default();
    assert!(body.contains("ping") && body.contains("errors"), "{body}");
    let skipped = logs.find(
        nest_rs_graphql::TARGET,
        "unreachable resolver skipped from the GraphQL schema",
    );
    let ping = skipped
        .iter()
        .find(|line| line.field("host").as_deref() == Some("PingResolver"))
        .unwrap_or_else(|| panic!("the host is not said: {skipped:?}"));
    assert_eq!(ping.level, "warn");

    let logs = LogCapture::install();
    let app = App::builder()
        .module::<HandRegisteredPingOnTheLoopback>()
        .build()
        .await
        .expect("the module boots");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();
    logs.expect_none(target::APP, UNSERVED);
    logs.expect_none(
        target::APP,
        "host not served: its edge does not mount an instance",
    );
}

#[module(imports = [GraphqlModule])]
struct GraphqlAlone;

/// Following that line serves the seed: the edge reads what the app reaches, and
/// a seed is reached.
#[tokio::test]
async fn a_resolver_seeded_beside_its_module_is_served() {
    let app = TestApp::builder()
        .provide(PingResolver)
        .module::<GraphqlAlone>()
        .build()
        .await
        .expect("the module boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    resp.assert_json(serde_json::json!({ "data": { "ping": "pong" } }))
        .await;
}
