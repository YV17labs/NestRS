//! [`AbilityGuard<F>`] — request-scoped bridge from the authenticated actor to
//! the [`Ability`] the enforcement layers read. Generic over
//! the app's [`AbilityFactory`].

use std::sync::Arc;

use nest_rs_core::{
    Edge, Layer, MissingAmbientError, Posture, UnitContext, async_trait, injectable,
};
use nest_rs_guards::__private::anonymous_denial;
use nest_rs_guards::{Denial, GrantedScopes, Guard, GuardPhase, PrincipalClaim};

use crate::gate::reason;
use crate::{Ability, AbilityBuilder, AbilityFactory, MalformedRuleError};

/// Bind after the auth guard: `#[use_guards(AuthnGuard, AbilityGuard<AuthzAbility>)]`.
/// `F::Actor` is the principal the authentication guard attached; its absence
/// on a non-public route is a `500` (an authn guard must run first). On a
/// `#[public]` unit of any edge, and on an in-band endpoint (`/graphql`) whose
/// operations gate themselves, the guard builds an Ability for the anonymous
/// (visitor) actor — see the dev's `AbilityFactory` to define visitor rules —
/// and for the actor when a guard ahead of it identified one, on a unit as on a
/// route.
///
/// The ability is attached for the rest of the request: on every edge, the reads
/// it scopes run where the unit's response mask arms them, and nowhere else.
///
/// `AuthzGuard` is not a framework type: an app aliases it once at its authz
/// module root, `pub type AuthzGuard = AbilityGuard<AuthzAbility>;`.
#[injectable]
#[derive(Layer)]
pub struct AbilityGuard<F: AbilityFactory> {
    #[inject]
    factory: Arc<F>,
}

impl<F: AbilityFactory> AbilityGuard<F> {
    /// The rules of the actor a guard ahead of this one identified, if any.
    fn actors(&self, cx: &UnitContext<'_>) -> Option<Result<Ability, MalformedRuleError>> {
        let actor = cx.ambient::<F::Actor>()?;
        // Absent ⇒ the principal is not scope-aware and no rule is scope-gated.
        let granted = cx
            .ambient::<GrantedScopes>()
            .map(|granted| granted.shared());
        let mut builder = AbilityBuilder::new().with_granted_scopes(granted);
        self.factory.define(&actor, &mut builder);
        Some(builder.build())
    }

    /// A caller no credential backs: the factory's visitor branch decides.
    fn visitor(&self) -> Result<Ability, MalformedRuleError> {
        // No credential, so no scopes — `Some(empty)`, not `None`: a scope-gated
        // visitor rule is withheld, never granted to everyone.
        let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
        self.factory.define_visitor(&mut builder);
        // Marked anonymous, so a posture's gate can refuse a visitor.
        builder.build_visitor()
    }
}

/// Whether `cx` is a federation root field: its site reads no posture, so nothing
/// after this guard would refuse a visitor.
#[cfg(feature = "graphql")]
fn states_no_posture(cx: &UnitContext<'_>) -> bool {
    use nest_rs_graphql::{FieldRole, GraphqlView};
    cx.switch_to::<GraphqlView>()
        .is_some_and(|field| field.role() == FieldRole::Federation)
}

/// Every other edge's unit answers to its posture.
#[cfg(not(feature = "graphql"))]
fn states_no_posture(_cx: &UnitContext<'_>) -> bool {
    false
}

/// `built` attached for the rest of the unit. A malformed rule fails construction
/// (fail-closed): the unit is denied rather than handed an ability whose denial
/// evaporates.
fn attach(
    cx: &mut UnitContext<'_>,
    built: Result<Ability, MalformedRuleError>,
) -> Result<(), Denial> {
    match built {
        Ok(ability) => {
            cx.attach(crate::context::AmbientAbility::new(ability));
            Ok(())
        }
        Err(err) => Err(Denial::internal("authorization rules are misconfigured")
            .with_reason(reason::RULES_MISCONFIGURED)
            .with_source(err)),
    }
}

/// Layer-System impl — bound after the authentication guard, on a host or in an
/// endpoint's `EndpointGuards`.
#[async_trait]
impl<F: AbilityFactory> Guard for AbilityGuard<F> {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let built = match self.actors(cx) {
            Some(built) => built,
            // `#[public]` or in band: no authenticated actor.
            None if matches!(cx.reflector().posture(), Posture::Public | Posture::InBand) => {
                self.visitor()
            }
            // Off a `#[public]` or in-band site, an actor is an earlier guard's
            // work: its absence is a wiring defect.
            None => return Err(MissingAmbientError::of::<F::Actor>().into()),
        };
        attach(cx, built)
    }

    /// Read the ability the unit's request attached; a visitor's reaches no
    /// federation root field, which states no posture its grant could answer to.
    /// With none, a `#[public]` unit takes its actor's rules or the visitor's, as a
    /// `#[public]` route does; any other is refused, for what its request ran.
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        if let Some(ability) = crate::context::carried(cx) {
            if ability.is_visitor() && states_no_posture(cx) {
                return Err(anonymous_denial());
            }
            return Ok(());
        }
        if cx.reflector().posture() == Posture::Public {
            let built = self.actors(cx).unwrap_or_else(|| self.visitor());
            return attach(cx, built);
        }
        let refused = Denial::unauthorized(match cx.edge() {
            _ if cx.ambient::<F::Actor>().is_some() => {
                "no ambient ability — the caller authenticated, and no ability guard ran at its request"
            }
            Some(Edge::Graphql) => {
                "no ambient ability — authentication did not run on the GraphQL operation"
            }
            Some(Edge::Ws) => "no ambient ability — WS connection did not authenticate",
            Some(Edge::Mcp) => "no ambient ability — the MCP operation did not authenticate",
            _ => "no ambient ability — the unit's request did not authenticate",
        });
        Err(refused.with_reason(reason::NO_AMBIENT_ABILITY))
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }

    /// Its visitor branch decides a caller with none.
    fn expected_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<F::Actor>().or_visitor())
    }

    fn __attaches_ability(&self) -> bool {
        true
    }

    /// None: a request is refused here only for a wiring defect, a `500`. The
    /// `403` an ability earns is answered by what reads it — the route's class
    /// gate, the row its `bind` loads, its service.
    fn denial_statuses(&self, _public: bool) -> &'static [u16] {
        &[]
    }

    /// Every refusal is filed on `nest_rs::authz`.
    fn target(&self) -> &'static str {
        crate::TARGET
    }
}

/// `AbilityGuard` checks HTTP: [`check_http`](Guard::check_http) attaches the
/// caller's ability. Declared under every transport feature, since each one
/// runs it on an HTTP request: a `#[controller]` binds it, a `#[gateway]`
/// struct on its upgrade, `/graphql` and `/mcp` on theirs.
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_http::HttpView> for AbilityGuard<F> {}

/// `AbilityGuard` checks GraphQL: [`check`](Guard::check) refuses a gated
/// operation no ability reaches. Declared so a resolver may bind it.
#[cfg(feature = "graphql")]
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_graphql::GraphqlView> for AbilityGuard<F> {}

/// And WebSocket messages, for the same reason: `check` refuses a gated message
/// whose connection never authenticated.
#[cfg(feature = "ws")]
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_ws::WsView> for AbilityGuard<F> {}

/// And MCP operations: [`check`](Guard::check) refuses a gated operation no
/// ability reaches.
#[cfg(feature = "mcp")]
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_mcp::McpView> for AbilityGuard<F> {}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::__private::{
        HandlerDeclaration, MetaLevels, declare_handler, declare_unit, new_unit_context,
    };
    use nest_rs_core::operation_log::Kind;
    use nest_rs_core::{AmbientValue, Handler, UnitView, with_ambient};
    use nest_rs_http::__private::{attached, check_request};
    use nest_rs_http::Request;

    use super::*;
    use crate::__private::{AmbientAbility, install_ability};
    use crate::{Ability, AbilityBuilder, Action};

    mod post {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "posts")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub published: bool,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    // `Post` points at `post::Entity`, so naming `comment::Entity` as the
    // related side trips the `Deny` sentinel `build` refuses.
    mod comment {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "comments")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub post_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {
            #[sea_orm(
                belongs_to = "super::post::Entity",
                from = "Column::PostId",
                to = "super::post::Column::Id"
            )]
            Post,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// The principal an authentication guard would have attached.
    #[derive(Clone)]
    struct Caller;

    impl AmbientValue for Caller {}

    struct NoRules;

    impl AbilityFactory for NoRules {
        type Actor = Caller;
        fn define(&self, _actor: &Caller, _builder: &mut AbilityBuilder) {}
    }

    fn guard() -> AbilityGuard<NoRules> {
        AbilityGuard {
            factory: Arc::new(NoRules),
        }
    }

    struct Routes;

    static PUBLIC_ROUTE: Handler = declare_handler(HandlerDeclaration {
        unit: nest_rs_http::unit::REQUEST,
        host: TypeId::of::<Routes>(),
        host_name: "Routes",
        method: "list",
        address: "/posts",
        posture: Posture::Public,
        meta: MetaLevels::default,
    });

    static GATED_ROUTE: Handler = declare_handler(HandlerDeclaration {
        unit: nest_rs_http::unit::REQUEST,
        host: TypeId::of::<Routes>(),
        host_name: "Routes",
        method: "create",
        address: "/posts",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// Run `guard`'s HTTP entry on a request to `route`: the ability it
    /// attached, or its denial.
    async fn ability_on<F: AbilityFactory>(
        guard: &AbilityGuard<F>,
        route: &'static Handler,
    ) -> Result<Option<Arc<Ability>>, Denial> {
        let mut req = Request::builder().finish();
        check_request(route, [("AbilityGuard", guard as &dyn Guard)], &mut req).await?;
        Ok(attached::<AmbientAbility>(&req).and_then(|held| held.ability()))
    }

    #[tokio::test]
    async fn the_default_visitor_branch_grants_nothing() {
        let ability = ability_on(&guard(), &PUBLIC_ROUTE)
            .await
            .expect("a public route admits the anonymous caller")
            .expect("the guard attaches an ability");
        assert!(
            !ability.can_class(Action::Read, TypeId::of::<post::Entity>()),
            "the default `define_visitor` must grant nothing",
        );
    }

    #[tokio::test]
    async fn a_visitor_grant_reaches_the_request() {
        struct PublicReads;

        impl AbilityFactory for PublicReads {
            type Actor = Caller;
            fn define(&self, _actor: &Caller, _builder: &mut AbilityBuilder) {}
            fn define_visitor(&self, ability: &mut AbilityBuilder) {
                ability
                    .can(Action::Read, post::Entity)
                    .when(|p| p.eq(post::Column::Published, true));
            }
        }

        let guard = AbilityGuard {
            factory: Arc::new(PublicReads),
        };
        let ability = ability_on(&guard, &PUBLIC_ROUTE)
            .await
            .expect("a public route admits the anonymous caller")
            .expect("the guard attaches an ability");
        assert!(
            ability.can_class(Action::Read, TypeId::of::<post::Entity>()),
            "the visitor branch's grant must reach the request",
        );
        assert!(
            ability.is_visitor(),
            "the ability must carry that no principal backs it — the GraphQL gate \
             reads this to keep a visitor grant out of an `#[authorize]` operation",
        );
    }

    #[tokio::test]
    async fn an_authenticated_ability_is_not_a_visitors() {
        let ability = with_ambient(Arc::new(Caller), ability_on(&guard(), &GATED_ROUTE))
            .await
            .expect("an authenticated actor builds an ability")
            .expect("the guard attaches an ability");
        assert!(
            !ability.is_visitor(),
            "an actor-backed ability is never the visitor's",
        );
    }

    #[tokio::test]
    async fn a_gated_route_with_no_actor_is_a_wiring_500() {
        let Err(denial) = ability_on(&guard(), &GATED_ROUTE).await else {
            panic!("no authentication guard ran, so the route is refused");
        };
        assert_eq!(denial.http_status(), 500);
    }

    /// Its one refusal of a request is that wiring `500`, which no guard
    /// declares; a `403` is answered by what reads the ability it attaches.
    #[test]
    fn it_declares_no_status_of_its_own_on_any_route() {
        assert!(guard().denial_statuses(true).is_empty());
        assert!(guard().denial_statuses(false).is_empty());
    }

    #[tokio::test]
    async fn a_malformed_visitor_rule_denies_instead_of_degrading() {
        struct MalformedVisitor;

        impl AbilityFactory for MalformedVisitor {
            type Actor = Caller;
            fn define(&self, _actor: &Caller, _builder: &mut AbilityBuilder) {}
            fn define_visitor(&self, ability: &mut AbilityBuilder) {
                ability.can(Action::Read, comment::Entity).when(|p| {
                    p.related::<comment::Entity, _>(comment::Relation::Post, |c| {
                        c.eq(comment::Column::Id, 1)
                    })
                });
            }
        }

        let mut req = Request::builder().finish();
        let guard = AbilityGuard {
            factory: Arc::new(MalformedVisitor),
        };
        let denial = check_request(
            &PUBLIC_ROUTE,
            [("AbilityGuard", &guard as &dyn Guard)],
            &mut req,
        )
        .await
        .expect_err("a malformed visitor rule must deny");
        assert_eq!(denial.http_status(), 500);
        assert!(
            attached::<AmbientAbility>(&req).is_none(),
            "a denied request must carry no ability at all",
        );
    }

    /// The rules the guard's `check` attached on the unit `handler` declares.
    async fn checked(handler: &'static Handler) -> Result<Arc<Ability>, Denial> {
        let mut view = Message;
        let mut cx = new_unit_context(handler, &mut view, None, None);
        guard().check(&mut cx).await?;
        Ok(crate::context::carried(&cx).expect("the guard attaches an ability"))
    }

    #[tokio::test]
    async fn a_public_unit_with_an_actor_ahead_takes_the_actors_rules() {
        let ability = with_ambient(Arc::new(Caller), checked(&PUBLIC_MESSAGE))
            .await
            .expect("a public unit admits its caller");
        assert!(
            !ability.is_visitor(),
            "a caller a guard identified takes its own rules, as on a route",
        );
    }

    #[tokio::test]
    async fn a_public_unit_with_no_actor_takes_the_visitors_rules() {
        let ability = checked(&PUBLIC_MESSAGE)
            .await
            .expect("a public unit admits a visitor");
        assert!(ability.is_visitor());
    }

    #[tokio::test]
    async fn a_gated_unit_with_an_actor_and_no_ability_names_the_ability_guard() {
        let Err(denial) = with_ambient(Arc::new(Caller), checked(&PING)).await else {
            panic!("nothing attached an ability at its request");
        };
        assert_eq!(denial.http_status(), 401);
        assert_eq!(denial.reason(), Some("no_ambient_ability"));
        assert!(
            denial.message().contains("ability guard"),
            "authentication ran, so the sentence names what did not: {}",
            denial.message(),
        );
    }

    struct Gateway;

    static PING: Handler = declare_handler(HandlerDeclaration {
        unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
        host: TypeId::of::<Gateway>(),
        host_name: "Gateway",
        method: "ping",
        address: "ping",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    struct Message;

    impl UnitView for Message {}

    static PUBLIC_MESSAGE: Handler = declare_handler(HandlerDeclaration {
        unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
        host: TypeId::of::<Gateway>(),
        host_name: "Gateway",
        method: "open",
        address: "open",
        posture: Posture::Public,
        meta: MetaLevels::default,
    });

    // A message whose upgrade attached no ability carries none: the
    // per-message guard must deny.
    #[tokio::test]
    async fn ws_message_without_ambient_ability_is_denied() {
        let mut view = Message;
        let mut cx = new_unit_context(&PING, &mut view, None, None);
        let guard = guard();
        let denial = guard
            .check(&mut cx)
            .await
            .expect_err("missing ambient ability must deny");
        assert_eq!(denial.http_status(), 401);
        assert_eq!(denial.reason(), Some("no_ambient_ability"));
        assert_eq!(
            guard.target(),
            crate::TARGET,
            "the edge files it on the authorization target"
        );
    }

    #[tokio::test]
    async fn ws_message_with_ambient_ability_passes() {
        let ability: Arc<Ability> =
            Arc::new(AbilityBuilder::new().build().expect("empty ability builds"));
        install_ability(ability, async {
            let mut view = Message;
            let mut cx = new_unit_context(&PING, &mut view, None, None);
            guard()
                .check(&mut cx)
                .await
                .expect("seeded ability admits the message");
        })
        .await;
    }
}
