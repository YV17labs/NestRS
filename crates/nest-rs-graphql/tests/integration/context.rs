//! Per-request context bridge: a value an HTTP guard attaches to the request
//! reaches a GraphQL resolver — end-to-end through the harness.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Context;
use nest_rs_graphql::{GraphqlContextSeed, GraphqlModule, resolver};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;
use poem::Request;

#[derive(Clone)]
struct RequestTag(String);

#[injectable]
#[derive(Default)]
struct TagGuard;

impl Layer for TagGuard {}

#[async_trait]
impl Guard for TagGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        req.extensions_mut().insert(RequestTag("hello".into()));
        Ok(())
    }
}

#[resolver]
struct TagResolver;

nest_rs_graphql::inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || Some(std::any::TypeId::of::<TagResolver>()),
        seed: |req, _container, gql| match req.extensions().get::<RequestTag>() {
            Some(tag) => gql.data(tag.clone()),
            None => gql,
        },
    }
}

#[resolver]
impl TagResolver {
    #[query]
    #[public]
    async fn tag(&self, ctx: &Context<'_>) -> String {
        ctx.data_opt::<RequestTag>()
            .map(|t| t.0.clone())
            .unwrap_or_else(|| "none".into())
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [TagGuard, TagResolver])]
struct GraphqlTestModule;

#[tokio::test]
async fn resolver_reads_a_per_request_value_bridged_from_the_poem_request() {
    let app = TestApp::builder()
        .module::<GraphqlTestModule>()
        .use_guards_global([guard::<TagGuard>()])
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ tag }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let tag = json
        .value()
        .object()
        .get("data")
        .object()
        .get("tag")
        .string();
    assert_eq!(tag, "hello");
}
