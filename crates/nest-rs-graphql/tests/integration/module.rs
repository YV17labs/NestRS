//! Covers `src/module.rs` — a setup imported twice mounts `/graphql` once,
//! the module attaches the HTTP transport it mounts on, a resolver the app
//! reaches without it is named at run, even beside the transport, and one it
//! serves is not, and a container no app sealed mounts no schema.

use nest_rs_core::{App, AppLifecycle, Discovery, Module, TransportContribution, module, target};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{HttpConfig, HttpEndpointMeta, HttpModule};
use nest_rs_testing::{LogCapture, TestApp};

/// What `App::run` files for a host whose edge's transport the app does not attach.
const UNSERVED: &str = "host not served: its edge's transport is not attached";

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
#[module(imports = [GraphqlModule::for_root(None)], providers = [PingResolver])]
struct ConfiguringPingModule;

/// A feature declaring the dependency bare.
#[module(imports = [GraphqlModule], providers = [PingResolver])]
struct BarePingModule;

#[module(imports = [GraphqlModule::for_root(None), ConfiguringPingModule])]
struct TwiceConfiguredApp;

#[module(imports = [BarePingModule, GraphqlModule::for_root(None)])]
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
    let app = App::builder()
        .module::<ConfiguringPingModule>()
        .build()
        .await
        .expect("the module boots");

    let transports: Vec<String> = Discovery::new(app.container())
        .meta::<TransportContribution>()
        .iter()
        .map(|contribution| contribution.meta.name().to_owned())
        .collect();
    assert_eq!(transports, ["HttpTransport"]);
}

/// A hand-written adapter that forgets the endpoint.
#[module(providers = [PingResolver])]
struct PingGraphqlModule;

#[tokio::test]
async fn a_resolver_the_app_reaches_without_the_endpoint_is_named_at_run() {
    let logs = LogCapture::install();

    App::new::<PingGraphqlModule>()
        .expect("the module boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.level, "warn");
    assert_eq!(unserved.field("edge").as_deref(), Some("graphql"));
    assert_eq!(unserved.field("host").as_deref(), Some("PingResolver"));
    assert_eq!(
        unserved.field("module").as_deref(),
        Some("PingGraphqlModule")
    );
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("nest_rs::graphql::GraphqlModule")
    );
}

/// The resolver's adapter beside the transport, on a free loopback port.
#[module(imports = [HttpModule::for_root(loopback()), BarePingModule])]
struct ServedPingModule;

/// The transport attached, and the adapter forgetting the endpoint.
#[module(imports = [HttpModule::for_root(loopback()), PingGraphqlModule])]
struct EndpointlessPingModule;

fn loopback() -> HttpConfig {
    HttpConfig {
        host: "127.0.0.1".into(),
        port: 0,
        ..HttpConfig::default()
    }
}

/// Runs the app `M` composes until it serves: `App::run` names an unserved
/// host before it turns ready, so the run is dropped there.
async fn run_until_ready<M: Module + 'static>() {
    let app = App::builder()
        .module::<M>()
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
}

#[tokio::test]
async fn a_resolver_the_endpoint_serves_is_not_named_at_run() {
    let logs = LogCapture::install();

    run_until_ready::<ServedPingModule>().await;

    logs.expect_one(target::APP, "app ready");
    logs.expect_none(target::APP, UNSERVED);
}

#[tokio::test]
async fn a_resolver_reached_beside_the_transport_without_the_endpoint_is_named_at_run() {
    let logs = LogCapture::install();

    run_until_ready::<EndpointlessPingModule>().await;

    logs.expect_one(target::APP, "app ready");
    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.field("edge").as_deref(), Some("graphql"));
    assert_eq!(unserved.field("host").as_deref(), Some("PingResolver"));
    assert_eq!(
        unserved.field("module").as_deref(),
        Some("PingGraphqlModule")
    );
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("nest_rs::graphql::GraphqlModule")
    );
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
        playground: true,
        introspection: true,
        ..nest_rs_graphql::GraphqlConfig::default()
    })],
    providers = [PingResolver],
)]
struct OpenedApp;

/// A pin opening the playground and introspection outside a dev profile is
/// honoured, and the boot mounting them says so once for each.
#[test]
fn a_pin_opening_the_surfaces_in_production_is_said_at_boot() {
    let logs = in_production(|| async {
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

/// Runs `boot` in a process that declares the production profile, which the
/// boot reads off the process environment `figment::Jail` scopes.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn in_production<F: std::future::Future>(boot: impl FnOnce() -> F) -> F::Output {
    let mut out = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(nest_rs_config::Environment::var_name(), "production");
        out = Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(boot()),
        );
        Ok(())
    });
    out.expect("the jailed boot ran")
}
