//! The global guard pool's **HTTP** check, run in-band by a *fallback* endpoint
//! guard.
//!
//! `/graphql` and `/mcp` are both `EdgePosture::Exempt`, so no guard runs in
//! front of them. **With no operation guard registered**, each seeds its
//! fallback from here: resolve the pool at mount, run every guard's
//! `check_http` in order, stop at the first denial — identical on both, so it
//! lives here once and each transport keeps only its own [`Denial`] mapping.
//!
//! **A registered bridge replaces the fallback, and then this never runs.** It
//! owns the request half itself (the canonical bridge runs its own authn and
//! authz guards), so a pooled guard's `check_http` is not executed for that
//! transport at all — which is the shape a `use_guards_global([ThrottlerGuard])`
//! beside an `AppMcpGuard` has, and why the docs qualify the fallback rather
//! than describing it as the pool's edge.
//!
//! Either way this is only the **request** half. A pooled guard's *operation*
//! check (`check_graphql` / `check_mcp`) runs in the per-operation chain, which
//! folds the same pool at the site where an operation exists to be checked, and
//! runs whether or not a bridge is registered.

use nest_rs_core::Container;
use nest_rs_core::layer_chain::ResolvedLayer;
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

    /// `true` when the pool resolved to nothing. `/mcp`'s default is closed, so
    /// its guard checks this rather than letting an empty chain read as "every
    /// guard passed" — the builder only seeds the fallback for a non-empty
    /// pool, but `resolve` drops specs it cannot resolve, so emptiness here is
    /// not the same question the builder answered.
    #[cfg(feature = "mcp")]
    pub(crate) fn is_empty(&self) -> bool {
        self.chain.is_empty()
    }

    /// Run the pool, returning the first [`Denial`] **as the guard raised it**
    /// so the caller's mapping keeps its status (a pooled throttler's `429`
    /// stays a `429`).
    ///
    /// Nothing is logged here: each guard logs its own denial at the source
    /// layer, and HTTP's `RouteShaper` doesn't re-log a pooled denial either.
    pub(crate) async fn check(&self, req: &mut Request) -> Result<(), Denial> {
        for entry in &self.chain {
            // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
            // blanket would nest a second boxed future per check.
            entry.layer.as_ref().check_http(req).await?;
        }
        Ok(())
    }
}
