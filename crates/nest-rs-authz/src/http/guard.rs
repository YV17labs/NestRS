//! [`AbilityGuard<F>`] — request-scoped bridge from the authenticated actor to
//! the [`Ability`](crate::Ability) the enforcement layers read. Generic over
//! the app's [`AbilityFactory`].

use std::sync::Arc;

use nest_rs_core::{HandlerMetadata, Layer, injectable};
use nest_rs_guards::{Denial, GrantedScopes, Guard, GuardPhase, PrincipalClaim};
use nest_rs_http::{Reflector, async_trait};
use nest_rs_ws::WsClient;
use poem::Request;
use serde_json::Value;

use crate::{AbilityBuilder, AbilityFactory, current_ability};

#[cfg(feature = "graphql")]
use nest_rs_graphql::GraphqlOperationContext;

/// Bind after the auth guard: `#[use_guards(AuthnGuard, AbilityGuard<AppAbility>)]`.
/// `F::Actor` is read from request extensions; its absence on a non-public
/// route is a `500` (an authn guard must run first). On a `#[public]`
/// route the guard builds an Ability for the anonymous (visitor) actor —
/// see the dev's `AbilityFactory` to define visitor rules.
///
/// **`AuthzGuard` is not a framework type.** Apps define a project alias once
/// in their authz adapter, e.g. `pub type AuthzGuard = AbilityGuard<AppAbility>;`
/// in `features/authz/http/guard.rs`. Import that alias from your feature crate,
/// not from `nest_rs_authz`.
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
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        // Build against a *borrowed* actor: the rules are read from it and never
        // outlive this block, so cloning the principal (claims, role list, …)
        // on every request bought nothing. The borrow ends before the insert.
        // What the credential was granted, published by the authn guard. Absent
        // ⇒ the principal is not scope-aware and no rule is scope-gated; the
        // overwhelmingly common case, and the reason this costs a non-OAuth app
        // nothing.
        let granted = req
            .extensions()
            .get::<GrantedScopes>()
            .map(GrantedScopes::shared);
        let built = match req.extensions().get::<F::Actor>() {
            Some(actor) => {
                let mut builder = AbilityBuilder::new().with_granted_scopes(granted);
                self.factory.define(actor, &mut builder);
                Some(builder.build())
            }
            // `#[public]`: no authenticated actor, so the factory's *visitor*
            // branch decides. It grants nothing unless the app overrode
            // `define_visitor`, which keeps an anonymous read fail-closed by
            // default while making a genuinely public resource expressible.
            // The result flows through the same match as the authenticated
            // branch: a malformed visitor rule denies rather than degrading.
            None if Reflector::new(req).is_public() => {
                // The anonymous caller holds no credential, so it carries no
                // scopes — `Some(empty)`, not `None`. A `define_visitor` rule
                // gated on a scope is therefore withheld rather than granted to
                // everyone, which is the fail-closed reading of the pair.
                let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
                self.factory.define_visitor(&mut builder);
                // `build_visitor`, not `build`: the ability carries the fact
                // that no principal backs it, so a transport declaring posture
                // *per operation* rather than per route (GraphQL) can refuse an
                // `#[authorize]` operation it would otherwise let a visitor
                // grant satisfy.
                Some(builder.build_visitor())
            }
            None => None,
        };
        match built {
            // A malformed rule fails construction (fail-closed): deny the
            // request rather than install an ability whose denial evaporates.
            Some(Ok(ability)) => {
                req.extensions_mut().insert(Arc::new(ability));
                Ok(())
            }
            Some(Err(err)) => {
                tracing::error!(
                    target: "nest_rs::authz",
                    error = %err,
                    "ability construction failed — denying the request",
                );
                Err(Denial::internal("authorization rules are misconfigured"))
            }
            None => {
                tracing::warn!(
                    target: "nest_rs::authz",
                    actor_type = std::any::type_name::<F::Actor>(),
                    "ability guard denied: no authenticated actor and route is not public",
                );
                Err(Denial::internal(
                    "AbilityGuard requires an authentication guard to run first",
                ))
            }
        }
    }

    #[cfg(feature = "graphql")]
    async fn check_graphql(&self, _op: &GraphqlOperationContext<'_>) -> Result<(), Denial> {
        if current_ability().is_none() {
            tracing::warn!(
                target: "nest_rs::authz",
                transport = "graphql",
                "authorization denied: no ambient ability",
            );
            return Err(Denial::unauthorized(
                "no ambient ability — authentication did not run on the GraphQL operation",
            ));
        }
        Ok(())
    }

    async fn check_ws_message(
        &self,
        _client: &WsClient,
        event: &str,
        _data: &Value,
    ) -> Result<(), Denial> {
        if current_ability().is_none() {
            tracing::warn!(
                target: "nest_rs::authz",
                transport = "ws",
                event = %event,
                "authorization denied: no ambient ability",
            );
            return Err(Denial::unauthorized(
                "no ambient ability — WS connection did not authenticate",
            ));
        }
        Ok(())
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }

    fn expected_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<F::Actor>())
    }
}

/// `AbilityGuard` checks HTTP: [`check_http`](Guard::check_http) installs the
/// caller's ability on the request. Declared so a `#[controller]` may bind it —
/// and so may a `#[gateway]` struct, whose guards run on the upgrade.
impl<F: AbilityFactory> nest_rs_guards::HttpGuard for AbilityGuard<F> {}

/// `AbilityGuard` checks GraphQL: [`check_graphql`](Guard::check_graphql) refuses
/// an operation no bridge installed an ability for. Declared so a resolver may
/// bind it — the marker is what a `#[use_guards]` on a `#[resolver]` requires.
#[cfg(feature = "graphql")]
impl<F: AbilityFactory> nest_rs_guards::GraphqlGuard for AbilityGuard<F> {}

/// And WebSocket messages, for the same reason: `check_ws_message` refuses a
/// message whose connection never authenticated.
impl<F: AbilityFactory> nest_rs_guards::WsGuard for AbilityGuard<F> {}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Public;

    use super::*;
    use crate::{Ability, AbilityBuilder, Action, with_ability};

    mod post {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "posts")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub published: bool,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    // A second entity so a rule can carry a *malformed* relation predicate: the
    // relation `Post` points at `post::Entity`, so naming `comment::Entity` as
    // the related side trips the `Deny` sentinel `build` refuses.
    mod comment {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "comments")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub post_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {
            #[sea_orm(
                belongs_to = "super::post::Entity",
                from = "Column::PostId",
                to = "super::post::Column::Id"
            )]
            Post,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    struct NoRules;

    impl AbilityFactory for NoRules {
        type Actor = ();
        fn define(&self, _actor: &(), _builder: &mut AbilityBuilder) {}
    }

    fn guard() -> AbilityGuard<NoRules> {
        AbilityGuard {
            factory: Arc::new(NoRules),
        }
    }

    fn public_request() -> Request {
        let mut req = Request::default();
        req.extensions_mut().insert(Public);
        req
    }

    /// The ability the guard attached, or `None` when it attached nothing.
    fn attached(req: &Request) -> Option<Arc<Ability>> {
        req.extensions().get::<Arc<Ability>>().cloned()
    }

    // The fail-closed floor of `define_visitor`: a factory that does not
    // override it must leave the visitor with nothing granted, so opening a
    // route with `#[public]` can never widen what the app declared.
    #[tokio::test]
    async fn the_default_visitor_branch_grants_nothing() {
        let mut req = public_request();
        guard()
            .check_http(&mut req)
            .await
            .expect("a public route admits the anonymous caller");

        let ability = attached(&req).expect("the guard attaches an ability");
        assert!(
            !ability.can_class(Action::Read, TypeId::of::<post::Entity>()),
            "the default `define_visitor` must grant nothing",
        );
    }

    #[tokio::test]
    async fn a_visitor_grant_reaches_the_request() {
        struct PublicReads;

        impl AbilityFactory for PublicReads {
            type Actor = ();
            fn define(&self, _actor: &(), _builder: &mut AbilityBuilder) {}
            fn define_visitor(&self, ability: &mut AbilityBuilder) {
                ability
                    .can(Action::Read, post::Entity)
                    .when(|p| p.eq(post::Column::Published, true));
            }
        }

        let mut req = public_request();
        AbilityGuard {
            factory: Arc::new(PublicReads),
        }
        .check_http(&mut req)
        .await
        .expect("a public route admits the anonymous caller");

        let ability = attached(&req).expect("the guard attaches an ability");
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
        let mut req = Request::default();
        req.extensions_mut().insert(());
        guard()
            .check_http(&mut req)
            .await
            .expect("an authenticated actor builds an ability");

        assert!(
            !attached(&req)
                .expect("the guard attaches an ability")
                .is_visitor(),
            "an actor-backed ability is never the visitor's",
        );
    }

    // The visitor branch flows through the same fail-closed match as the
    // authenticated one: a malformed rule denies with a 500 instead of
    // degrading to a deny-all ability that reads as an ordinary empty result.
    #[tokio::test]
    async fn a_malformed_visitor_rule_denies_instead_of_degrading() {
        struct MalformedVisitor;

        impl AbilityFactory for MalformedVisitor {
            type Actor = ();
            fn define(&self, _actor: &(), _builder: &mut AbilityBuilder) {}
            fn define_visitor(&self, ability: &mut AbilityBuilder) {
                ability.can(Action::Read, comment::Entity).when(|p| {
                    p.related::<comment::Entity, _>(comment::Relation::Post, |c| {
                        c.eq(comment::Column::Id, 1)
                    })
                });
            }
        }

        let mut req = public_request();
        let denial = AbilityGuard {
            factory: Arc::new(MalformedVisitor),
        }
        .check_http(&mut req)
        .await
        .expect_err("a malformed visitor rule must deny");

        assert_eq!(denial.http_status(), 500);
        assert!(
            attached(&req).is_none(),
            "a denied request must carry no ability at all",
        );
    }

    // The WS-auth fail-secure carry-over: a gateway module that imported
    // `AuthzHttpModule` instead of `AuthzWsModule` boots (the upgrade guards
    // resolve) but registers no `SocketContext`, so no ability is re-seeded
    // around message handlers. The per-message guard must then deny — not
    // silently pass an unauthenticated message through.
    #[tokio::test]
    async fn ws_message_without_ambient_ability_is_denied() {
        let client = WsClient::for_test();
        let denial = guard()
            .check_ws_message(&client, "ping", &Value::Null)
            .await
            .expect_err("missing ambient ability must deny");
        assert_eq!(denial.http_status(), 401);
    }

    #[tokio::test]
    async fn ws_message_with_ambient_ability_passes() {
        let client = WsClient::for_test();
        let ability: Arc<Ability> =
            Arc::new(AbilityBuilder::new().build().expect("empty ability builds"));
        with_ability(ability, async {
            guard()
                .check_ws_message(&client, "ping", &Value::Null)
                .await
                .expect("seeded ability admits the message");
        })
        .await;
    }
}
