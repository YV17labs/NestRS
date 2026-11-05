//! `AbilityGuard`'s unit entry on a GraphQL operation (`src/guard.rs`), through the
//! real decorators: bound at the resolver alone, with nothing at the `/graphql`
//! request, a `#[public]` operation reads the visitor's rules and a gated one still
//! fails closed; a visitor's ability reaches no federation root field.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Delete, Read};
use nest_rs_core::{AmbientValue, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::{EndpointGuards, HttpView};
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
    #[public(Read, widget::Entity, unmasked)]
    async fn readable(&self) -> GqlResult<String> {
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

/// Identifies a caller at the `/graphql` request, as an authentication guard does,
/// and attaches no ability.
#[injectable]
#[derive(Default)]
struct IdentifiesAtTheRequest;

impl Layer for IdentifiesAtTheRequest {}

#[async_trait]
impl Guard for IdentifiesAtTheRequest {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<Actor>())
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        if HttpView::of(cx)?.request().header("x-role").is_some() {
            cx.attach(Actor);
        }
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for IdentifiesAtTheRequest {}

#[module(
    imports = [GraphqlModule],
    providers = [
        Rules,
        AuthzGuard,
        IdentifiesAtTheRequest,
        EndpointGuards<GraphqlView, (IdentifiesAtTheRequest,)>,
        VisitorResolver,
    ],
)]
struct IdentifiedModule;

#[tokio::test]
async fn a_public_operation_under_its_resolvers_ability_guard_reads_its_actors_rules() {
    let app = TestApp::for_module::<IdentifiedModule>()
        .await
        .expect("the resolver boots");

    let readable = query(&app, "reader", "{ readable }").await;
    assert_eq!(
        readable["data"]["readable"], "caller may not delete",
        "{readable}"
    );

    let refused = query(&app, "", "{ readable }").await;
    assert_eq!(
        refused["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "a visitor has no such grant: {refused}",
    );

    let gated = query(&app, "reader", "{ gated }").await;
    assert_eq!(
        gated["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{gated}",
    );
    assert!(
        gated["errors"][0]["message"]
            .as_str()
            .is_some_and(|said| said.contains("no ability guard ran at its request")),
        "authentication ran, so the sentence names what did not: {gated}",
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
