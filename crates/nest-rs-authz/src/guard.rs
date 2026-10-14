//! [`AbilityGuard<F>`] — request-scoped bridge from the authenticated actor to
//! the [`Ability`] the enforcement layers read. Generic over
//! the app's [`AbilityFactory`].

use std::sync::Arc;

use nest_rs_core::{Edge, Layer, Posture, UnitContext, injectable};
use nest_rs_guards::{Denial, GrantedScopes, Guard, GuardPhase, PrincipalClaim};
use nest_rs_http::async_trait;

use crate::gate::{Refusal, reason};
use crate::{Ability, AbilityBuilder, AbilityFactory};

/// Bind after the auth guard: `#[use_guards(AuthnGuard, AbilityGuard<AuthzAbility>)]`.
/// `F::Actor` is the principal the authentication guard attached; its absence
/// on a non-public route is a `500` (an authn guard must run first). On a
/// `#[public]` route, and on an in-band endpoint (`/graphql`) whose operations
/// gate themselves, the guard builds an Ability for the anonymous (visitor)
/// actor — see the dev's `AbilityFactory` to define visitor rules.
///
/// The ability is attached for the rest of the request: on a route, the reads
/// it scopes run where the route's response mask arms them, and nowhere else.
///
/// `AuthzGuard` is not a framework type: an app aliases it once at its authz
/// module root, `pub type AuthzGuard = AbilityGuard<AuthzAbility>;`.
#[injectable]
pub struct AbilityGuard<F: AbilityFactory> {
    #[inject]
    factory: Arc<F>,
}

impl<F: AbilityFactory> Layer for AbilityGuard<F> {}

/// Layer-System impl — global registration via
/// `use_guards_global([..., guard::<AuthzGuard>()])` is the canonical path.
#[async_trait]
impl<F: AbilityFactory> Guard for AbilityGuard<F> {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        // Absent ⇒ the principal is not scope-aware and no rule is scope-gated.
        let granted = cx
            .ambient::<GrantedScopes>()
            .map(|granted| granted.shared());
        let built = match cx.ambient::<F::Actor>() {
            Some(actor) => {
                let mut builder = AbilityBuilder::new().with_granted_scopes(granted);
                self.factory.define(&actor, &mut builder);
                Some(builder.build())
            }
            // `#[public]` or in band: no authenticated actor, so the factory's
            // visitor branch decides.
            None if matches!(cx.reflector().posture(), Posture::Public | Posture::InBand) => {
                // No credential, so no scopes — `Some(empty)`, not `None`: a
                // scope-gated visitor rule is withheld, never granted to everyone.
                let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
                self.factory.define_visitor(&mut builder);
                // `build_visitor` marks the ability anonymous, so a per-operation
                // gate (GraphQL) can refuse an `#[authorize]` operation.
                Some(builder.build_visitor())
            }
            None => None,
        };
        match built {
            // A malformed rule fails construction (fail-closed): deny the
            // request rather than attach an ability whose denial evaporates.
            Some(Ok(ability)) => {
                cx.attach(ability);
                Ok(())
            }
            Some(Err(err)) => {
                tracing::error!(
                    target: crate::TARGET,
                    error = %nest_rs_core::error_message(&err),
                    "ability construction failed — denying the request",
                );
                Err(Denial::internal("authorization rules are misconfigured"))
            }
            None => {
                tracing::warn!(
                    target: crate::TARGET,
                    actor_type = std::any::type_name::<F::Actor>(),
                    "ability guard denied: no authenticated actor and route is not public",
                );
                Err(Denial::internal(
                    "AbilityGuard requires an authentication guard to run first",
                ))
            }
        }
    }

    /// Refuse a unit no ability reaches: the request that carried it did not
    /// authenticate, or nothing re-established the ability for it.
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        if cx.ambient::<Ability>().is_some() {
            return Ok(());
        }
        let edge = cx.edge();
        crate::gate::warn_denied(Refusal {
            reason: Some(reason::NO_AMBIENT_ABILITY),
            event: (edge == Some(Edge::Ws)).then(|| cx.handler().address()),
            ..Refusal::on(edge.map_or("unit", Edge::as_str))
        });
        Err(Denial::unauthorized(match edge {
            Some(Edge::Graphql) => {
                "no ambient ability — authentication did not run on the GraphQL operation"
            }
            Some(Edge::Ws) => "no ambient ability — WS connection did not authenticate",
            Some(Edge::Mcp) => "no ambient ability — the MCP operation did not authenticate",
            _ => "no ambient ability — the unit's request did not authenticate",
        }))
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }

    fn expected_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<F::Actor>())
    }
}

/// `AbilityGuard` checks HTTP: [`check_http`](Guard::check_http) attaches the
/// caller's ability. Declared under every transport feature, since each one
/// runs it on an HTTP request: a `#[controller]` binds it, a `#[gateway]`
/// struct on its upgrade, `/graphql` and `/mcp` on theirs.
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_http::HttpView> for AbilityGuard<F> {}

/// `AbilityGuard` checks GraphQL: [`check`](Guard::check) refuses an operation
/// no ability reaches. Declared so a resolver may bind it.
#[cfg(feature = "graphql")]
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_graphql::GraphqlView> for AbilityGuard<F> {}

/// And WebSocket messages, for the same reason: `check` refuses a message
/// whose connection never authenticated.
#[cfg(feature = "ws")]
impl<F: AbilityFactory> nest_rs_guards::GuardFor<nest_rs_ws::WsView> for AbilityGuard<F> {}

/// And MCP operations: [`check`](Guard::check) refuses an operation no ability
/// reaches.
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
    use poem::Request;

    use super::*;
    use crate::{AbilityBuilder, Action, with_ability};

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
        let mut req = Request::default();
        check_request(route, [("AbilityGuard", guard as &dyn Guard)], &mut req).await?;
        Ok(attached::<Ability>(&req))
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

        let mut req = Request::default();
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
            attached::<Ability>(&req).is_none(),
            "a denied request must carry no ability at all",
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

    // A gateway importing `AuthzHttpModule` instead of `AuthzWsModule` seeds no
    // ability around messages: the per-message guard must deny.
    #[tokio::test]
    async fn ws_message_without_ambient_ability_is_denied() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut view = Message;
        let mut cx = new_unit_context(&PING, &mut view, None, None);
        let denial = guard()
            .check(&mut cx)
            .await
            .expect_err("missing ambient ability must deny");
        assert_eq!(denial.http_status(), 401);

        let event = logs.expect_one("nest_rs::authz", "authorization denied");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("transport").as_deref(), Some("ws"));
        assert_eq!(event.field("event").as_deref(), Some("ping"));
        assert_eq!(
            event.field("reason").as_deref(),
            Some("no_ambient_ability"),
            "{:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn ws_message_with_ambient_ability_passes() {
        let ability: Arc<Ability> =
            Arc::new(AbilityBuilder::new().build().expect("empty ability builds"));
        with_ability(ability, async {
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
