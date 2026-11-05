//! `AbilityGuard`'s unit entry on a GraphQL operation (`src/guard.rs`), through the
//! real decorators: bound at the resolver alone, with nothing at the `/graphql`
//! request, a `#[public]` operation reads the visitor's rules and a gated one still
//! fails closed; a visitor's ability reaches no federation root field.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Delete, Read};
use nest_rs_core::{AmbientValue, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_guards::GlobalGuards;
use nest_rs_testing::TestApp;

use super::rules_read;
use crate::graphql::query;
use crate::widget;

#[derive(Clone)]
struct Actor;

impl AmbientValue for Actor {}

#[injectable]
#[derive(Default)]
struct Rules;

impl AbilityFactory for Rules {
    type Actor = Actor;

    fn define(&self, _actor: &Self::Actor, ability: &mut AbilityBuilder) {
        ability.can(Action::Read, widget::Entity);
    }

    fn define_visitor(&self, ability: &mut AbilityBuilder) {
        ability.can(Action::Delete, widget::Entity);
    }
}

type AuthzGuard = AbilityGuard<Rules>;

#[resolver]
#[use_guards(AuthzGuard)]
struct VisitorResolver;

#[operations]
impl VisitorResolver {
    #[query]
    #[public(Delete, widget::Entity, unmasked)]
    async fn open(&self) -> GqlResult<String> {
        Ok(rules_read())
    }

    #[query]
    #[authorize(Read, widget::Entity, unmasked)]
    async fn gated(&self) -> GqlResult<String> {
        Ok("reached".to_owned())
    }
}

#[module(imports = [GraphqlModule], providers = [Rules, AuthzGuard, VisitorResolver])]
struct VisitorModule;

#[tokio::test]
async fn a_public_operation_under_its_resolvers_ability_guard_reads_the_visitors_rules() {
    let app = TestApp::for_module::<VisitorModule>()
        .await
        .expect("the resolver boots");

    let open = query(&app, "", "{ open }").await;
    assert_eq!(open["data"]["open"], "visitor may delete", "{open}");

    let gated = query(&app, "", "{ gated }").await;
    assert_eq!(
        gated["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{gated}",
    );
}

#[module(
    imports = [GraphqlModule::for_root(GraphqlConfig {
        federation: true,
        ..GraphqlConfig::default()
    })],
    providers = [Rules, AuthzGuard, GlobalGuards<(AuthzGuard,)>],
)]
struct PooledSubgraphApp;

/// No guard of this pool identifies anyone, so a visitor's ability is all that
/// reaches `_service`, whose field states no posture for its grant to answer to.
#[tokio::test]
async fn a_visitors_ability_reaches_no_federation_root_field() {
    let app = TestApp::for_module::<PooledSubgraphApp>()
        .await
        .expect("a subgraph under a pooled ability guard boots");

    let sdl = query(&app, "", "{ _service { sdl } }").await;
    assert!(sdl["data"]["_service"].is_null(), "{sdl}");
    assert_eq!(
        sdl["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{sdl}",
    );
}
