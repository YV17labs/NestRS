//! The global guard pool's **HTTP** check, run in band at the request by
//! `/graphql` and `/mcp`.
//!
//! Both are `EdgePosture::Exempt`: whenever the pool is non-empty, each runs it
//! through its slot, with the registered bridge, if any, run inside it. A
//! bridge adds to the pool and never replaces it. Either way this is only the
//! request half; a pooled guard's operation check runs in the per-operation
//! chain.

use std::any::TypeId;

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

    /// Split the pool at the first guard a bridge covers, the bridge running
    /// every guard `covers` names itself: the guards listed before that one,
    /// then the rest, every covered guard left to the bridge. Run head, bridge,
    /// tail, and the bridge sits where the pool placed the authentication it
    /// carries, ahead of a pooled guard reading the principal; with nothing
    /// covered it runs after the whole pool.
    pub(crate) fn split_at_covered(self, covers: &[TypeId]) -> (Self, Self) {
        let covered = |entry: &ResolvedLayer<dyn Guard>| covers.contains(&entry.type_id);
        let mut head = self.chain;
        let at = head.iter().position(covered).unwrap_or(head.len());
        let mut tail = head.split_off(at);
        tail.retain(|entry| !covered(entry));
        (Self { chain: head }, Self { chain: tail })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::{Layer, LayerSite};

    use super::*;

    struct First;
    struct Covered;
    struct Middle;
    struct AlsoCovered;
    struct Last;

    macro_rules! admit_all {
        ($($guard:ident),*) => {$(
            impl Layer for $guard {}
            impl Guard for $guard {}
        )*};
    }

    admit_all!(First, Covered, Middle, AlsoCovered, Last);

    fn entry<G: Guard>(guard: G) -> ResolvedLayer<dyn Guard> {
        ResolvedLayer {
            type_id: TypeId::of::<G>(),
            name: std::any::type_name::<G>(),
            source: LayerSite::Global,
            layer: Arc::new(guard),
        }
    }

    fn pool() -> GlobalPoolChain {
        GlobalPoolChain {
            chain: vec![
                entry(First),
                entry(Covered),
                entry(Middle),
                entry(AlsoCovered),
                entry(Last),
            ],
        }
    }

    fn ids(chain: &GlobalPoolChain) -> Vec<TypeId> {
        chain.chain.iter().map(|entry| entry.type_id).collect()
    }

    #[test]
    fn the_bridge_sits_at_the_first_guard_it_covers_and_runs_every_covered_one() {
        let covers = [TypeId::of::<Covered>(), TypeId::of::<AlsoCovered>()];
        let (head, tail) = pool().split_at_covered(&covers);
        assert_eq!(ids(&head), [TypeId::of::<First>()]);
        assert_eq!(ids(&tail), [TypeId::of::<Middle>(), TypeId::of::<Last>()]);
    }

    #[test]
    fn a_bridge_covering_nothing_in_the_pool_runs_after_it() {
        let (head, tail) = pool().split_at_covered(&[TypeId::of::<u8>()]);
        assert_eq!(ids(&head).len(), 5);
        assert!(ids(&tail).is_empty());
    }

    #[test]
    fn a_bridge_covering_the_whole_pool_runs_alone() {
        let covers = [
            TypeId::of::<First>(),
            TypeId::of::<Covered>(),
            TypeId::of::<Middle>(),
            TypeId::of::<AlsoCovered>(),
            TypeId::of::<Last>(),
        ];
        let (head, tail) = pool().split_at_covered(&covers);
        assert!(ids(&head).is_empty());
        assert!(ids(&tail).is_empty());
    }
}
