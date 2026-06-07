//! Per-route guard that runs a [`Strategy`](super::Strategy) and attaches the principal.

use std::sync::Arc;

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{Reflector, async_trait};
use poem::Request;

use crate::passport::{Outcome, Strategy};

#[injectable]
pub struct AuthGuard<S: Strategy> {
    #[inject]
    strategy: Arc<S>,
}

impl<S: Strategy> AuthGuard<S> {
    /// Construct with an already-resolved strategy (container or tests).
    pub fn new(strategy: Arc<S>) -> Self {
        Self { strategy }
    }
}

impl<S: Strategy> Layer for AuthGuard<S> {}

/// Layer-System impl — registers globally via
/// `App::builder().use_guards_global([guard::<AuthGuard>(), ...])` and is the
/// canonical path. `check_graphql` and `check_ws_message` keep the no-op
/// defaults because the GraphQL POST and WS upgrade are both HTTP requests
/// this `check_http` covers at the connection edge.
///
/// On a `#[public]` route, the guard runs but never rejects: it still
/// authenticates if a token is present (attaches the principal so a
/// downstream policy guard can see who's calling) but silently lets an
/// anonymous request through. Visitor-rule policy belongs in the
/// authorization layer, not in `AuthGuard`.
#[async_trait]
impl<S: Strategy> Guard for AuthGuard<S> {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let strategy = std::any::type_name::<S>();
        let is_public = Reflector::new(req).is_public();
        match self.strategy.authenticate(req).await {
            Ok(Outcome::Authenticated(principal)) => {
                tracing::debug!(target: "nest_rs::auth", strategy, "authenticated");
                req.extensions_mut().insert(principal);
                Ok(())
            }
            Ok(Outcome::Challenge(_)) if is_public => {
                tracing::debug!(target: "nest_rs::auth", strategy, "no credential on a public route — letting it through");
                Ok(())
            }
            Ok(Outcome::Challenge(_)) => {
                tracing::debug!(target: "nest_rs::auth", strategy, "authentication challenge issued");
                Err(Denial::unauthorized("authentication challenge"))
            }
            Err(_) if is_public => {
                tracing::debug!(target: "nest_rs::auth", strategy, "authentication failed on a public route — letting it through");
                Ok(())
            }
            Err(error) => {
                tracing::warn!(target: "nest_rs::auth", strategy, error = %error, "authentication failed");
                Err(Denial::unauthorized(error.to_string()))
            }
        }
    }
}

