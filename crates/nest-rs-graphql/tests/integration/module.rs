//! Covers `src/module.rs` — a setup imported twice mounts `/graphql` once,
//! and a container no app sealed mounts no schema.

use nest_rs_core::{Discovery, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::HttpEndpointMeta;
use nest_rs_testing::TestApp;

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

/// A mount has no `Result`: the schema's discovery files its refusal on the
/// container, and the boot fails on it once its transports are configured.
#[tokio::test]
async fn the_schema_mounted_over_a_container_no_app_sealed_is_refused_naming_its_reader() {
    // Built by hand, so no wiring step refuses first.
    let container = nest_rs_core::Container::builder()
        .provide(nest_rs_graphql::GraphqlConfig::default())
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
