//! The per-request context bridge: a value an HTTP guard attaches to the request
//! reaches a GraphQL resolver, driven end-to-end through the in-process harness.
//! This is the seam GraphQL authorization is built on (the actor's `Ability`).

use nestrs_core::module;
use nestrs_graphql::async_graphql::Context;
use nestrs_graphql::{resolver, ContextSeed, GraphqlModule};
use nestrs_http::{async_trait, Guard, HttpTransport};
use nestrs_testing::TestApp;
use poem::{Request, Response};

/// A per-request value an HTTP guard attaches; the bridge forwards it into the
/// GraphQL context for the resolver to read.
#[derive(Clone)]
struct RequestTag(String);

/// A global guard (runs before routing) attaches the value to the poem request.
struct TagGuard;

#[async_trait]
impl Guard for TagGuard {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        req.extensions_mut().insert(RequestTag("hello".into()));
        Ok(())
    }
}

#[resolver]
struct TagResolver;

// Forward `RequestTag` from the poem request into the GraphQL context. Gated on
// `TagResolver` so it fires whenever the resolver is reachable from the running
// app — the same mechanism that gates `forward_principal!`.
nestrs_graphql::inventory::submit! {
    ContextSeed {
        owner_type_id: || Some(std::any::TypeId::of::<TagResolver>()),
        seed: |req, _container, gql| match req.extensions().get::<RequestTag>() {
            Some(tag) => gql.data(tag.clone()),
            None => gql,
        },
    }
}

#[resolver]
impl TagResolver {
    /// Reads the bridged per-request value from the context (`ctx: &Context` is
    /// forwarded natively by `#[query]`, no macro support needed).
    #[query]
    async fn tag(&self, ctx: &Context<'_>) -> String {
        ctx.data_opt::<RequestTag>()
            .map(|t| t.0.clone())
            .unwrap_or_else(|| "none".into())
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [TagResolver])]
struct GraphqlTestModule;

#[tokio::test]
async fn resolver_reads_a_per_request_value_bridged_from_the_poem_request() {
    let app = TestApp::builder()
        .module::<GraphqlTestModule>()
        .http(HttpTransport::new().guard(TagGuard))
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
