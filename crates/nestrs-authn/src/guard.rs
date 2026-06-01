//! [`AuthGuard<S>`] — the per-route guard that runs a [`Strategy`] and, on
//! success, makes the authenticated principal available to the rest of the
//! request. The analog of NestJS's `AuthGuard('name')`.

use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_http::{async_trait, Guard};
use poem::{IntoResponse, Request, Response};

use crate::strategy::{Outcome, Strategy};

/// Generic over the app's [`Strategy`], so the wiring — run the strategy, attach
/// the principal, or short-circuit — is identical for every scheme. Resolve the
/// strategy from the container, so it is itself an `#[injectable]` provider.
///
/// Bind it per route, usually behind a `type` alias:
///
/// ```ignore
/// pub type JwtAuthGuard = nestrs_authn::AuthGuard<JwtStrategy>;
/// // ...
/// #[use_guards(JwtAuthGuard, AppAbilityGuard)]
/// ```
///
/// On [`Outcome::Authenticated`] the principal is inserted into the request
/// extensions, where a later guard (`AbilityGuard`) or the `Ctx<Principal>`
/// extractor reads it; on [`Outcome::Challenge`] the request short-circuits with
/// the strategy's response (a redirect or a `401`).
#[injectable]
pub struct AuthGuard<S: Strategy> {
    #[inject]
    strategy: Arc<S>,
}

#[async_trait]
impl<S: Strategy> Guard for AuthGuard<S> {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        let strategy = std::any::type_name::<S>();
        match self.strategy.authenticate(req).await {
            Ok(Outcome::Authenticated(principal)) => {
                tracing::debug!(target: "nestrs::auth", strategy, "authenticated");
                req.extensions_mut().insert(principal);
                Ok(())
            }
            Ok(Outcome::Challenge(response)) => {
                tracing::debug!(target: "nestrs::auth", strategy, "authentication challenge issued");
                Err(response)
            }
            Err(error) => {
                tracing::warn!(target: "nestrs::auth", strategy, error = %error, "authentication failed");
                Err(error.into_response())
            }
        }
    }
}
