//! Covers `src/module.rs` — a setup imported twice mounts `/graphql` once.

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
