//! What `GET` on the GraphQL path answers when it opens no socket: the
//! playground, or a method refusal naming what the path serves — and which
//! development surfaces the bare import opens under the suites' profile.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::{Method, StatusCode, header};
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

#[module(imports = [GraphqlModule::for_root(GraphqlConfig::default())], providers = [PingResolver])]
struct NoPlaygroundModule;

#[module(imports = [GraphqlModule::for_root(None)], providers = [PingResolver])]
struct BareModule;

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

/// RFC 9110 §15.5.6: a `405` lists what the path serves; a plain `GET` is not
/// a GraphQL request here, a `POST` is.
#[tokio::test]
async fn a_plain_get_without_the_playground_is_405_naming_post() {
    let app = TestApp::for_module::<NoPlaygroundModule>()
        .await
        .expect("boots");

    for method in [Method::GET, Method::HEAD] {
        let resp = app.http().request(method.clone(), "/graphql").send().await;

        resp.assert_status(StatusCode::METHOD_NOT_ALLOWED);
        resp.assert_content_type("application/problem+json");
        resp.assert_header(header::ALLOW, "POST");
        if method == Method::GET {
            let problem: serde_json::Value = resp.json().await.value().deserialize();
            assert_eq!(problem["code"], "METHOD_NOT_ALLOWED", "{problem}");
        }
    }
}

/// The suites declare the test profile, whose `Config::defaults()` opens both
/// development surfaces on the bare import, as it opens the documentation.
#[tokio::test]
async fn the_bare_import_serves_the_playground_and_introspection_in_the_test_profile() {
    let app = TestApp::for_module::<BareModule>().await.expect("boots");

    let page = app.http().get("/graphql").send().await;
    page.assert_status_is_ok();
    page.assert_content_type("text/html; charset=utf-8");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ __schema { queryType { name } } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body: serde_json::Value = resp.json().await.value().deserialize();
    assert_eq!(
        body["data"]["__schema"]["queryType"]["name"], "Query",
        "{body}"
    );
}
