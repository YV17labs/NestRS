//! Per-route guard that runs a [`Strategy`](super::Strategy) and attaches the principal.

use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_http::{Guard, async_trait};
use poem::{IntoResponse, Request, Response};

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
