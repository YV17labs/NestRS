//! [`AbilityGuard<F>`] — the request-scoped bridge that turns the authenticated
//! actor into the [`Ability`](crate::Ability) the enforcement layers read.
//!
//! It is generic over the app's [`AbilityFactory`], so the only app-specific
//! parts (the policy and the actor type) stay in the app; the wiring — read the
//! actor, run the factory, attach the ability — is the same everywhere and lives
//! here.

use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_http::{async_trait, Guard};
use poem::http::StatusCode;
use poem::{Request, Response};

use crate::{AbilityBuilder, AbilityFactory};

/// Bind it per route after the authentication guard, parameterized by the app's
/// factory: `#[use_guards(AuthGuard, AbilityGuard<AppAbility>)]`. It resolves the
/// factory from the container, builds the actor's [`Ability`](crate::Ability),
/// and stores it as `Arc<Ability>` for the [`Authorize`](super::Authorize)
/// extractor and handlers to read.
///
/// The actor (`F::Actor`) is read from the request extensions, so an
/// authentication guard must run first and insert it; its absence is a `500`
/// (the authn guard was not applied to this route — a wiring bug).
#[injectable]
pub struct AbilityGuard<F: AbilityFactory> {
    #[inject]
    factory: Arc<F>,
}

#[async_trait]
impl<F: AbilityFactory> Guard for AbilityGuard<F> {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        let Some(actor) = req.extensions().get::<F::Actor>().cloned() else {
            return Err(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("AbilityGuard requires an authentication guard to run first"));
        };
        let mut builder = AbilityBuilder::new();
        self.factory.define(&actor, &mut builder);
        req.extensions_mut().insert(Arc::new(builder.build()));
        Ok(())
    }
}
