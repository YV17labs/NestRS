//! GraphQL per-site chain helper. Emitted inline at the start of every
//! `#[query]` / `#[mutation]` / `#[field_resolver]` by `#[resolver]`.
//!
//! Composes the global + provider-scope + handler-scope chain, dedups by
//! `TypeId`, and runs `check_graphql`. WS has no inline runner: the
//! `#[messages]` macro composes its per-event guard table at gateway mount,
//! wrapping each guard via `GuardAsWsMessageCheck`.
//!
//! ## Composed once per site, like every other transport
//!
//! HTTP bakes its chain into a [`RouteShaper`](super::RouteShaper) at mount and
//! WS into an `EventLayerTable` at gateway mount; GraphQL has no mount seam it
//! could hang one on — the schema is built by `nest-rs-graphql`, which cannot
//! see [`Guard`] — so each site memoizes its own chain in a
//! [`GraphqlChainCell`] the `#[resolver]` macro emits as a `static` beside the
//! call. Composition (container lookups, dedup, sort) therefore happens once
//! per site, not once per resolution; the steady-state cost is one atomic load
//! and one `Arc` clone.
//!
//! The cell is keyed by [`ContainerId`] rather than blindly memoizing: a test
//! process serves several apps, and one app's guard chain must never gate
//! another's resolvers. Ids are never recycled, so a stale hit is impossible.

use std::any::TypeId;
use std::sync::{Arc, Mutex, OnceLock};

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain, dedup_bucket};
use nest_rs_core::{Container, ContainerId};
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};

use crate::Guard;
use crate::dispatch::denial_convert::denial_to_graphql_error;
use crate::dispatch::route_shaper::log_effective_chain;
use crate::dispatch::scoped_spec::{ScopedGuardSpec, resolve_global_guards, resolve_specs};

/// The scope-tagged guard declarations of one resolver site, as the
/// `#[resolver]` macro knows them. Read **once per site** — on the cache miss
/// that composes the chain — so building the `Vec`s costs nothing per request.
///
/// Macro-emitted, not public API.
#[doc(hidden)]
pub struct GraphqlChainSources {
    /// `#[use_guards(...)]` on the resolver struct.
    pub resolver: Vec<ScopedGuardSpec>,
    /// `#[use_guards(...)]` beside the operation.
    pub method: Vec<ScopedGuardSpec>,
    /// `#[force_guards(...)]` — replay these even when a broader scope has them.
    pub force: Vec<TypeId>,
}

/// One resolver site's composed guard chain, memoized per [`ContainerId`].
///
/// The `#[resolver]` macro emits one as a `static` per guarded operation and
/// hands it to [`run_layered_graphql_chain`].
///
/// Macro-emitted, not public API.
#[doc(hidden)]
#[derive(Default)]
pub struct GraphqlChainCell {
    /// The serving app's chain — the only entry a real process ever fills.
    primary: OnceLock<Cached>,
    /// Further apps sharing the process (integration tests build several).
    /// Allocated only when a second container reaches this site.
    extra: OnceLock<Mutex<Vec<Cached>>>,
}

struct Cached {
    container: ContainerId,
    chain: Arc<[ResolvedLayer<dyn Guard>]>,
}

impl GraphqlChainCell {
    /// An empty cell — `const` so the macro can put one in a `static`.
    pub const fn new() -> Self {
        Self {
            primary: OnceLock::new(),
            extra: OnceLock::new(),
        }
    }

    fn chain(
        &self,
        container: &Container,
        route_label: &str,
        sources: &dyn Fn() -> GraphqlChainSources,
    ) -> Arc<[ResolvedLayer<dyn Guard>]> {
        let id = container.id();
        let primary = self.primary.get_or_init(|| Cached {
            container: id,
            chain: compose(container, route_label, sources()),
        });
        if primary.container == id {
            return Arc::clone(&primary.chain);
        }

        // Another app in the same process. A poisoned lock must not deny
        // service — the vector holds only memoized values, so recovering it is
        // safe (worst case a chain composes twice).
        let mut slots = self
            .extra
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hit) = slots.iter().find(|c| c.container == id) {
            return Arc::clone(&hit.chain);
        }
        let chain = compose(container, route_label, sources());
        slots.push(Cached {
            container: id,
            chain: Arc::clone(&chain),
        });
        chain
    }
}

fn compose(
    container: &Container,
    route_label: &str,
    sources: GraphqlChainSources,
) -> Arc<[ResolvedLayer<dyn Guard>]> {
    let global = dedup_bucket(resolve_global_guards(container));
    let resolver = resolve_specs(container, &sources.resolver, LayerSite::Controller);
    let method = resolve_specs(container, &sources.method, LayerSite::Method);
    let chain = compose_chain::<dyn Guard>(global, resolver, method, &sources.force, route_label);
    log_effective_chain(route_label, "guards", &chain);
    chain.into()
}

/// GraphQL shaper helper. Called by `#[resolver]` at the start of every
/// resolver method. Dedups per-resolver guards against the global chain.
///
/// `cell` memoizes the composed chain for `container`; `sources` is consulted
/// only when it has to be composed (see the module docs).
///
/// GraphQL pipes ([`nest_rs_pipes::GlobalPipe::transform_graphql_variables`])
/// are not invoked here — variables live at the operation level, not per
/// resolver, so they run at the GraphQL transport's request entry
/// (`nest_rs_graphql::context` folds them over an operation's variables).
pub async fn run_layered_graphql_chain(
    ctx: &GraphqlContext<'_>,
    container: &Container,
    cell: &GraphqlChainCell,
    route_label: &str,
    sources: impl Fn() -> GraphqlChainSources,
) -> std::result::Result<(), GraphqlError> {
    let chain = cell.chain(container, route_label, &sources);
    for entry in chain.iter() {
        if let Err(denial) = entry.layer.check_graphql(ctx).await {
            // Structural floor mirroring `deny_http`: every denial visible at
            // warn+ regardless of what the individual guard logged.
            tracing::warn!(
                target: "nest_rs::layers",
                guard = entry.name,
                route = route_label,
                status = denial.http_status(),
                "guard denied the operation",
            );
            return Err(denial_to_graphql_error(denial));
        }
    }
    Ok(())
}
