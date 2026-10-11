//! `GraphqlAbilityBridge` scopes everything an operation runs, the DataLoader
//! batches it triggers included, with no batch binding listed anywhere.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::dataloader::DataLoader;
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult};
use nest_rs_graphql::{GraphqlModule, GraphqlOperationGuard, dataloader, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_testing::TestApp;

use super::query;

/// Stands in for the authentication slot of the bridge.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {}

impl HttpGuard for PassGuard {}

/// Stands in for `AbilityGuard`: a caller with an `x-role` header gets an
/// authenticated ability, anyone else the visitor's.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let builder = AbilityBuilder::new();
        let ability = match req.headers().get("x-role") {
            Some(_) => builder.build(),
            None => builder.build_visitor(),
        };
        req.extensions_mut()
            .insert(Arc::new(ability.expect("an empty rule set builds")));
        Ok(())
    }
}

impl HttpGuard for AbilityInjector {}

/// Answers each key with the ability its batch ran under.
#[injectable]
#[derive(Default)]
struct AbilityShelf;

#[dataloader]
impl AbilityShelf {
    async fn seen(&self, keys: &[i32]) -> HashMap<i32, String> {
        let seen = match current_ability() {
            Some(ability) if ability.is_visitor() => "visitor",
            Some(_) => "caller",
            None => "none",
        };
        keys.iter().map(|key| (*key, seen.to_owned())).collect()
    }
}

#[resolver]
struct AbilityShelfResolver;

#[operations]
impl AbilityShelfResolver {
    #[query]
    #[public]
    async fn batch_ability(&self, ctx: &Context<'_>, id: i32) -> GqlResult<String> {
        let loader = ctx.data::<DataLoader<AbilityShelfSeen>>()?;
        let Ok(found) = loader.load_one(id).await;
        Ok(found.unwrap_or_default())
    }
}

type TestOpGuard = GraphqlAbilityBridge<PassGuard, AbilityInjector>;

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        PassGuard,
        AbilityInjector,
        TestOpGuard as dyn GraphqlOperationGuard,
        AbilityShelf,
        AbilityShelfResolver,
    ],
)]
struct BatchAbilityModule;

#[tokio::test]
async fn a_batch_runs_under_the_ability_the_bridge_installed_for_its_request() {
    let app = TestApp::for_module::<BatchAbilityModule>()
        .await
        .expect("the schema boots");

    let caller = query(&app, "admin", "{ batchAbility(id: 1) }").await;
    assert_eq!(caller["data"]["batchAbility"], "caller", "{caller}");

    let visitor = query(&app, "", "{ batchAbility(id: 1) }").await;
    assert_eq!(visitor["data"]["batchAbility"], "visitor", "{visitor}");
}
