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
