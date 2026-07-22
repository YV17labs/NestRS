//! [`AbilityGuard<F>`] — request-scoped bridge from the authenticated actor to
//! the [`Ability`](crate::Ability) the enforcement layers read. Generic over
//! the app's [`AbilityFactory`].

use std::sync::Arc;

use nest_rs_core::{HandlerMetadata, Layer, injectable};
use nest_rs_guards::{Denial, Guard, GuardPhase, PrincipalClaim};
use nest_rs_http::{Reflector, async_trait};
use nest_rs_ws::WsClient;
use poem::Request;
use serde_json::Value;

use crate::{AbilityBuilder, AbilityFactory, current_ability};

#[cfg(feature = "graphql")]
use nest_rs_graphql::async_graphql::Context as GraphqlContext;

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
        let built = req.extensions().get::<F::Actor>().map(|actor| {
            let mut builder = AbilityBuilder::new();
            self.factory.define(actor, &mut builder);
            builder.build()
        });
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
            None if Reflector::new(req).is_public() => {
                // `#[public]`: no authenticated actor expected. Attach an
                // empty Ability so downstream layers (DbContext etc.) have
                // something to install, and visitor-scope reads end up
                // empty by default. A dev that wants visitor *rules*
                // grants them explicitly in their `AbilityFactory`'s
                // visitor branch — out of scope here. An empty builder has no
                // rules, so it never fails; fall back to a deny-all ability.
                req.extensions_mut()
                    .insert(Arc::new(AbilityBuilder::new().build().unwrap_or_default()));
                Ok(())
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
    async fn check_graphql(&self, _ctx: &GraphqlContext<'_>) -> Result<(), Denial> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ability, AbilityBuilder, with_ability};

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
