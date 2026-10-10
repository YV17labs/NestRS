//! [`GlobalPoolFederationGuard`] — the app-wide chain in front of `_service`
//! and `_entities`.
//!
//! Those root fields resolve above the merged root, out of reach of the chain
//! `#[operations]` emits. The pool is the whole chain here: a federation field
//! belongs to no resolver, so there is no `#[use_guards]` scope to compose.

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
    /// [`FederationGate`](nest_rs_graphql::__private::FederationGate).
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
                    // Same structural floor as `run_layered_graphql_chain`.
                    tracing::warn!(
                        target: nest_rs_core::target::LAYERS,
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
