//! [`GlobalPoolFederationGuard`] — the app-wide chain in front of `_service`
//! and `_entities`.
//!
//! Those two root fields are async-graphql's, resolved above the merged root, so
//! the chain `#[operations]` emits inside a resolver body never reaches them:
//! `_service` answered a `check_graphql` deny-all pool with the endpoint's whole
//! SDL, and `_entities` ran the chain once per representation rather than once
//! per field. `nest_rs_graphql`'s schema extension is the seam; this is what it
//! runs.
//!
//! **The pool is the whole chain here, and that is not a shortcut.** A
//! federation field belongs to no resolver — the router calls it on the schema —
//! so there is no `#[use_guards]` scope to compose and no posture to read. The
//! `#[entity]` bodies reached *through* `_entities` compose everything else, and
//! deliberately not the pool: it ran here.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_graphql::async_graphql::Error as GraphqlError;
use nest_rs_graphql::{BoxFuture, GraphqlFederationGuard, GraphqlOperationContext};

use crate::dispatch::denial_convert::denial_to_graphql_error;
use crate::dispatch::global_pool::GlobalPoolChain;

/// Runs the global guard pool against a federation root field.
pub struct GlobalPoolFederationGuard {
    pool: GlobalPoolChain,
}

impl GlobalPoolFederationGuard {
    /// The factory `use_guards_global` seeds as
    /// [`FederationGate`](nest_rs_graphql::FederationGate).
    pub fn factory(container: &Container) -> Arc<dyn GraphqlFederationGuard> {
        Arc::new(Self {
            pool: GlobalPoolChain::resolve(container, "POST /graphql (federation)"),
        })
    }
}

impl GraphqlFederationGuard for GlobalPoolFederationGuard {
    fn check<'a>(
        &'a self,
        operation: &'a GraphqlOperationContext<'a>,
    ) -> BoxFuture<'a, Result<(), GraphqlError>> {
        Box::pin(async move {
            match self.pool.check_operation(operation).await {
                Ok(()) => Ok(()),
                Err((name, denial)) => {
                    // Same structural floor as `run_layered_graphql_chain`: a
                    // denial is visible at warn+ whatever the guard itself
                    // logged, and this is the one site whose route label a
                    // reader would otherwise have to infer.
                    tracing::warn!(
                        target: "nest_rs::layers",
                        guard = name,
                        route = "POST /graphql (federation)",
                        field = operation.name(),
                        status = denial.http_status(),
                        "guard denied the federation field",
                    );
                    Err(denial_to_graphql_error(denial))
                }
            }
        })
    }
}
