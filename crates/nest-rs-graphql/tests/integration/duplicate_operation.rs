//! Two resolvers may not claim one operation name.
//!
//! Before the boot check, this composed in silence and the two halves of the
//! merge disagreed on the winner: the SDL published `contested(tag: String!)`
//! from the *last* registration while `resolve_field` returned from the *first*
//! member that answered, so a client sending the documented argument reached a
//! body that never saw it. Both orders follow `inventory::iter` — link order.
//!
//! The security half is why it is an error rather than a `warn`: `#[authorize]`
//! expands inside the operation's body, so the posture that runs is the one
//! belonging to whichever body won the dispatch, not the one the schema
//! documents.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_testing::TestApp;

#[resolver]
struct FirstContestedResolver;

#[operations]
impl FirstContestedResolver {
    #[query]
    #[public]
    async fn contested(&self) -> String {
        "first".into()
    }
}

#[resolver]
struct SecondContestedResolver;

#[operations]
impl SecondContestedResolver {
    #[query]
    #[public]
    async fn contested(&self, tag: String) -> String {
        format!("second:{tag}")
    }
}

#[module(providers = [FirstContestedResolver, SecondContestedResolver])]
struct ContestedModule;

#[module(imports = [GraphqlModule::for_root(None), ContestedModule])]
struct ContestedApp;

#[tokio::test]
async fn two_resolvers_claiming_one_operation_fail_the_boot() {
    let boot = TestApp::builder()
        .module::<ContestedApp>()
        .http(HttpTransport::new())
        .build()
        .await;
    let Err(err) = boot else {
        panic!("a contested operation name must not compose in silence");
    };

    let msg = format!("{err:#}");
    assert!(
        msg.contains("duplicate GraphQL operation name"),
        "the boot names the failure mode: {msg}"
    );
    // Naming *both* owners is the point: the loser is otherwise unreachable
    // with nothing in the logs to say which resolver lost.
    assert!(
        msg.contains("FirstContestedResolver") && msg.contains("SecondContestedResolver"),
        "the boot names both owners: {msg}"
    );
    assert!(
        msg.contains("contested"),
        "the boot names the contested operation: {msg}"
    );
}

#[resolver]
struct SoloQueryResolver;

#[operations]
impl SoloQueryResolver {
    #[query]
    #[public]
    async fn overlapping(&self) -> String {
        "query".into()
    }
}

#[resolver]
struct SoloMutationResolver;

#[operations]
impl SoloMutationResolver {
    #[mutation]
    #[public]
    async fn overlapping(&self) -> String {
        "mutation".into()
    }
}

#[module(providers = [SoloQueryResolver, SoloMutationResolver])]
struct OverlappingModule;

#[module(imports = [GraphqlModule::for_root(None), OverlappingModule])]
struct OverlappingApp;

#[tokio::test]
async fn a_query_and_a_mutation_may_share_a_name() {
    // Query and Mutation are two root objects, so one name in each is not a
    // collision — checking per schema rather than per root would refuse a
    // `createOrder` mutation beside a `createOrder` query preview.
    let app = TestApp::builder()
        .module::<OverlappingApp>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("one name per root is not a duplicate");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ overlapping }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
}
