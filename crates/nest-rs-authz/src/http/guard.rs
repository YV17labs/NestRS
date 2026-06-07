//! [`AbilityGuard<F>`] — request-scoped bridge from the authenticated actor to
//! the [`Ability`](crate::Ability) the enforcement layers read. Generic over
//! the app's [`AbilityFactory`].

use std::sync::Arc;

use nest_rs_core::{Layer, injectable};
use nest_rs_graphql::async_graphql::Context as GraphqlContext;
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{Reflector, async_trait};
use nest_rs_ws::WsClient;
use poem::Request;
use serde_json::Value;

use crate::{AbilityBuilder, AbilityFactory, current_ability};

/// Bind after the auth guard: `#[use_guards(AuthGuard, AbilityGuard<AppAbility>)]`.
/// `F::Actor` is read from request extensions; its absence on a non-public
/// route is a `500` (an authn guard must run first). On a `#[public]`
/// route the guard builds an Ability for the anonymous (visitor) actor —
/// see the dev's `AbilityFactory` to define visitor rules.
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
        match req.extensions().get::<F::Actor>().cloned() {
            Some(actor) => {
                let mut builder = AbilityBuilder::new();
                self.factory.define(&actor, &mut builder);
                req.extensions_mut().insert(Arc::new(builder.build()));
                Ok(())
            }
            None if Reflector::new(req).is_public() => {
                // `#[public]`: no authenticated actor expected. Attach an
                // empty Ability so downstream layers (DbContext etc.) have
                // something to install, and visitor-scope reads end up
                // empty by default. A dev that wants visitor *rules*
                // grants them explicitly in their `AbilityFactory`'s
                // visitor branch — out of scope here.
                req.extensions_mut().insert(Arc::new(AbilityBuilder::new().build()));
                Ok(())
            }
            None => Err(Denial::internal(
                "AbilityGuard requires an authentication guard to run first",
            )),
        }
    }

    async fn check_graphql(&self, _ctx: &GraphqlContext<'_>) -> Result<(), Denial> {
        if current_ability().is_none() {
            return Err(Denial::unauthorized(
                "no ambient ability — authentication did not run on the GraphQL operation",
            ));
        }
        Ok(())
    }

    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &Value,
    ) -> Result<(), Denial> {
        if current_ability().is_none() {
            return Err(Denial::unauthorized(
                "no ambient ability — WS connection did not authenticate",
            ));
        }
        Ok(())
    }
}

