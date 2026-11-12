//! `#[wire_enum(graphql)]`, core's decorator reaching this edge by its key: an enum
//! read as an argument and answered, with no database and no entity anywhere.

use nest_rs_core::{module, wire_enum};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;

#[wire_enum(graphql)]
pub(crate) enum Role {
    Admin,
    User,
}

#[resolver]
struct RoleResolver;

#[operations]
impl RoleResolver {
    #[query]
    #[public]
    async fn other(&self, role: Role) -> Role {
        match role {
            Role::Admin => Role::User,
            Role::User => Role::Admin,
        }
    }
}

#[module(imports = [GraphqlModule], providers = [RoleResolver])]
struct RoleModule;

#[tokio::test]
async fn a_graphql_wire_enum_crosses_an_operation_both_ways() {
    let app = TestApp::for_module::<RoleModule>().await.expect("boots");
    let query = serde_json::json!({ "query": "{ other(role: ADMIN) }" });
    app.http()
        .post("/graphql")
        .body_json(&query)
        .send()
        .await
        .assert_json(serde_json::json!({ "data": { "other": "USER" } }))
        .await;
}
