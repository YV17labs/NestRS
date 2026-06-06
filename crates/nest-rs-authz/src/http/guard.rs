//! [`AbilityGuard<F>`] — request-scoped bridge from the authenticated actor to
//! the [`Ability`](crate::Ability) the enforcement layers read. Generic over
//! the app's [`AbilityFactory`].

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_http::{Guard, async_trait};
use poem::http::StatusCode;
use poem::{Request, Response};

use crate::{AbilityBuilder, AbilityFactory};

/// Bind after the auth guard: `#[use_guards(AuthGuard, AbilityGuard<AppAbility>)]`.
/// `F::Actor` is read from request extensions; its absence is a `500` (an
/// authn guard must run first — a wiring bug).
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
