//! What `GET` on the GraphQL path answers when it opens no socket.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;

#[resolver]
struct PingResolver;

#[operations]
impl PingResolver {
    #[query]
    #[public]
    async fn ping(&self) -> bool {
        true
    }
}

#[module(
    imports = [GraphqlModule::for_root(GraphqlConfig {
        playground: true,
        ..GraphqlConfig::default()
    })],
    providers = [PingResolver],
)]
struct PlaygroundModule;

#[tokio::test]
async fn the_playground_is_html_naming_the_schemas_path() {
    let app = TestApp::for_module::<PlaygroundModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/graphql").send().await;

    resp.assert_status_is_ok();
    resp.assert_content_type("text/html; charset=utf-8");
    let html = resp.0.into_body().into_string().await.expect("a body");
    assert!(html.contains("GraphQL Playground"), "{html:.120}");
    assert!(html.contains("\"/graphql\""), "{html:.120}");
}
