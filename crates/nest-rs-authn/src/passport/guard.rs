//! Per-route guard that runs a [`Strategy`](super::Strategy) and attaches the principal.

use std::sync::Arc;

use nest_rs_core::{HandlerMetadata, Layer, injectable};
use nest_rs_guards::{Denial, Guard, GuardPhase, PrincipalClaim};
use nest_rs_http::{Reflector, async_trait};
use poem::Request;

use crate::error::AuthError;
use crate::passport::Strategy;

/// The authentication guard: runs a [`Strategy`] on the request, attaches the
/// resulting principal, and records `actor_id` on the span. Generic over the
/// strategy `S`. Bind it via `#[use_guards]` / `use_guards_global`; on a
/// `#[public]` route it authenticates opportunistically but never rejects.
#[injectable]
pub struct AuthnGuard<S: Strategy> {
    #[inject]
    strategy: Arc<S>,
}

impl<S: Strategy> AuthnGuard<S> {
    /// Construct with an already-resolved strategy (container or tests).
    pub fn new(strategy: Arc<S>) -> Self {
        Self { strategy }
    }
}

impl<S: Strategy> Layer for AuthnGuard<S> {}

/// Layer-System impl — registers globally via
/// `App::builder().use_guards_global([guard::<AuthnGuard>(), ...])` and is the
/// canonical path. `check_graphql` and `check_ws_message` keep the no-op
/// defaults because the GraphQL POST and WS upgrade are both HTTP requests
/// this `check_http` covers at the connection edge.
///
/// On a `#[public]` route, the guard authenticates opportunistically and does
/// not reject a *credential* failure: it attaches the principal when a token
/// verifies (so a downstream policy guard sees who is calling) and otherwise
/// continues anonymously — a rejected credential logged at `warn`, a plain
/// anonymous call at `debug`. Visitor-rule policy belongs in the authorization
/// layer, not in `AuthnGuard`. The one failure `#[public]` does **not** absorb
/// is [`AuthError::Unavailable`]: an unreachable identity store means the
/// credential was never evaluated, so the request fails closed with a 500
/// rather than being served as anonymous.
#[async_trait]
impl<S: Strategy> Guard for AuthnGuard<S> {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let strategy = std::any::type_name::<S>();
        let is_public = Reflector::new(req).is_public();
        match self.strategy.authenticate(req).await {
            Ok(principal) => {
                // Record the audit identity on the request span (the OTel
                // interceptor pre-declares `actor_id`) so every downstream
                // event — denials included — inherits who is calling.
                if let Some(actor_id) = crate::PrincipalIdentity::actor_id(&principal) {
                    tracing::Span::current().record("actor_id", actor_id.as_str());
                    tracing::debug!(target: "nest_rs::authn", strategy, actor_id, "authenticated");
                } else {
                    tracing::debug!(target: "nest_rs::authn", strategy, "authenticated");
                }
                req.extensions_mut().insert(principal);
                Ok(())
            }
            // The store could not be reached, so the presented credential was
            // never evaluated — neither "authenticated" nor "anonymous" is a
            // true answer. Fail closed on every route, `#[public]` included:
            // admitting the caller as anonymous would silently downgrade every
            // authenticated session for the duration of the outage.
            Err(error @ AuthError::Unavailable(_)) => {
                tracing::error!(
                    target: "nest_rs::authn",
                    strategy,
                    reason = error.reason(),
                    error = %error,
                    "authentication unavailable — identity store unreachable",
                );
                Err(Denial::internal(error.client_message()))
            }
            // A public route admits the anonymous caller, but a credential that
            // was *presented and rejected* is a security event: a forged or
            // expired token probing a public endpoint must leave a queryable
            // trace, not a `debug` line.
            Err(AuthError::MissingCredentials) if is_public => {
                tracing::debug!(target: "nest_rs::authn", strategy, "anonymous request on a public route");
                Ok(())
            }
            Err(error) if is_public => {
                tracing::warn!(
                    target: "nest_rs::authn",
                    strategy,
                    reason = error.reason(),
                    error = %error,
                    "rejected credential on a public route — continuing as anonymous",
                );
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    target: "nest_rs::authn",
                    strategy,
                    reason = error.reason(),
                    error = %error,
                    "authentication failed",
                );
                Err(Denial::unauthorized(error.client_message()))
            }
        }
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<S::Principal>())
    }
}
