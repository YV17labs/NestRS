//! `AbilityGuard`'s unit entry on a GraphQL operation (`src/guard.rs`), through the
//! real decorators: bound at the resolver alone, with nothing at the `/graphql`
//! request, a `#[public]` operation reads the visitor's rules, and a gated one or a
//! field resolver refuses the boot; a visitor's ability reaches no federation root
//! field.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Delete, Read};
use nest_rs_core::{AmbientValue, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::{EndpointGuards, HttpView};
use nest_rs_testing::TestApp;

use super::{assert_refused_at_the_request, rules_read};
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
}

#[resolver]
#[use_guards(AuthzGuard)]
struct GatedAtTheUnitResolver;

#[operations]
impl GatedAtTheUnitResolver {
    #[query]
    #[authorize(Read, widget::Entity, unmasked)]
    async fn gated_at_the_unit(&self) -> GqlResult<String> {
        Ok("reached".to_owned())
    }
}

#[module(imports = [GraphqlModule], providers = [Rules, AuthzGuard, GatedAtTheUnitResolver])]
struct GatedAtTheUnitModule;

/// The guard builds an ability at a `#[public]` unit alone: a gated one reads what its
/// request attached, so with nothing attaching it there every call would answer `401`.
#[tokio::test]
async fn a_gated_operation_whose_ability_guard_sits_at_the_unit_alone_refuses_the_boot() {
    assert_refused_at_the_request(
        TestApp::for_module::<GatedAtTheUnitModule>().await,
        "query gated_at_the_unit",
    );
}

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct GuardShelf {
    id: i32,
}

#[resolver]
#[use_guards(AuthzGuard)]
struct GuardShelfResolver;

#[operations]
impl GuardShelfResolver {
    #[query]
    #[public(Delete, widget::Entity, unmasked)]
    async fn guard_shelves(&self) -> GqlResult<Vec<GuardShelf>> {
        Ok(vec![GuardShelf { id: 1 }])
    }

    #[field_resolver]
    async fn label(&self, parent: &GuardShelf) -> String {
        format!("shelf {}", parent.id)
    }
}

#[module(imports = [GraphqlModule], providers = [Rules, AuthzGuard, GuardShelfResolver])]
struct FieldAtTheUnitModule;

/// A field resolver runs its resolver's guards and states no posture: the ability guard
/// there reads what the `/graphql` request attached, never what its root field built, so
/// with nothing attaching it at the request every call would answer `401`.
#[tokio::test]
async fn a_field_resolver_whose_ability_guard_sits_at_the_unit_alone_refuses_the_boot() {
    assert_refused_at_the_request(
        TestApp::for_module::<FieldAtTheUnitModule>().await,
        "GuardShelfResolver.label",
    );
}

#[module(
    imports = [GraphqlModule],
    providers = [
        Rules,
        AuthzGuard,
        EndpointGuards<GraphqlView, (AuthzGuard,)>,
        GuardShelfResolver,
    ],
)]
struct FieldAttachedAtTheRequestModule;

#[tokio::test]
async fn a_field_resolver_reads_the_ability_its_request_attached() {
    let app = TestApp::for_module::<FieldAttachedAtTheRequestModule>()
        .await
        .expect("the request attaches the ability the field reads");

    let shelves = query(&app, "", "{ guardShelves { id label } }").await;
    assert_eq!(
        shelves["data"]["guardShelves"][0]["label"], "shelf 1",
        "{shelves}"
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
}

#[module(
    imports = [GraphqlModule],
    providers = [
        Rules,
        AuthzGuard,
        IdentifiesAtTheRequest,
        EndpointGuards<GraphqlView, (IdentifiesAtTheRequest,)>,
        GatedAtTheUnitResolver,
    ],
)]
struct IdentifiedGatedModule;

/// Authentication at the request changes nothing: the ability is what the gated
/// operation reads there.
#[tokio::test]
async fn a_gated_operation_identified_at_its_request_and_authorized_at_the_unit_refuses_the_boot() {
    assert_refused_at_the_request(
        TestApp::for_module::<IdentifiedGatedModule>().await,
        "query gated_at_the_unit",
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
