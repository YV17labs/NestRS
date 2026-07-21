//! The global guard pool, resolved once and run in-band.
//!
//! `/graphql` and `/mcp` are both `EdgePosture::Exempt`, so each folds the
//! global pool into its own per-operation seam. The folding is identical —
//! resolve the pool at mount, run every guard in order, stop at the first
//! denial — so it lives here once and each transport keeps only its own
//! [`Denial`] mapping. A change to how a pooled denial is handled in-band then
//! cannot land on one transport and miss the other.

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
            entry.layer.check_http(req).await?;
        }
        Ok(())
    }
}
