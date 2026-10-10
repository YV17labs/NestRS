//! The global guard pool's **HTTP** check, run in-band by a *fallback* endpoint
//! guard.
//!
//! `/graphql` and `/mcp` are `EdgePosture::Exempt`: with no operation guard
//! registered, each seeds its fallback from here. A registered bridge replaces
//! the fallback, and then this never runs. Either way it is only the request
//! half; a pooled guard's operation check runs in the per-operation chain.

use nest_rs_core::__private::ResolvedLayer;
use nest_rs_core::Container;
use poem::Request;

use crate::Guard;
use crate::denial::Denial;
use crate::registry::GuardSpecs;

/// The resolved global guard pool for one `Exempt`-edge transport.
pub(crate) struct GlobalPoolChain {
    chain: Vec<ResolvedLayer<dyn Guard>>,
}

impl GlobalPoolChain {
    /// Resolve the pool eagerly — the container is final at mount. `label`
    /// names the site in the chain diagnostics (`"POST /mcp (operation)"`).
    pub(crate) fn resolve(container: &Container, label: &'static str) -> Self {
        let chain = container
            .get::<GuardSpecs>()
            .map(|specs| specs.resolve_chain(container, label))
            .unwrap_or_default();
        Self { chain }
    }

    /// `true` when the pool resolved to nothing. `/mcp`'s default is closed and
    /// `resolve` drops specs it cannot resolve, so its guard checks this rather
    /// than reading an empty chain as "every guard passed".
    #[cfg(feature = "mcp")]
    pub(crate) fn is_empty(&self) -> bool {
        self.chain.is_empty()
    }

    /// Run the pool, returning the first [`Denial`] **as the guard raised it**
    /// so the caller's mapping keeps its status (a pooled throttler's `429`
    /// stays a `429`).
    pub(crate) async fn check(&self, req: &mut Request) -> Result<(), Denial> {
        for entry in &self.chain {
            // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
            // blanket would nest a second boxed future per check.
            entry.layer.as_ref().check_http(req).await?;
        }
        Ok(())
    }

    /// Run the pool's **operation** half against a GraphQL operation, returning
    /// the denying guard's name beside its [`Denial`] for the caller to log.
    #[cfg(feature = "graphql")]
    pub(crate) async fn check_operation(
        &self,
        operation: &nest_rs_graphql::GraphqlOperationContext<'_>,
    ) -> Result<(), (&'static str, Denial)> {
        for entry in &self.chain {
            if let Err(denial) = entry.layer.as_ref().check_graphql(operation).await {
                return Err((entry.name, denial));
            }
        }
        Ok(())
    }
}
