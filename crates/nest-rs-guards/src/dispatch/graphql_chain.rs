//! GraphQL per-site chain runner. Emitted inline at the start of every
//! `#[query]` / `#[mutation]` / `#[subscription]` / `#[entity]` /
//! `#[field_resolver]` by `#[operations]`, whatever the method returns, which
//! names the site — `#[entity]` leaves the app-wide pool to the federation gate,
//! `#[field_resolver]` to the root field it resolves under, and a root field
//! folds it.
//!
//! The cell, the sources and the composition live in [`chain`](super::chain).

use nest_rs_core::Container;
use nest_rs_graphql::__private::FederationGate;
use nest_rs_graphql::GraphqlOperationContext;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};

use crate::dispatch::chain::{GlobalBucket, SiteChainCell, SiteChainSources};
use crate::dispatch::denial_convert::denial_to_graphql_error;

/// Which GraphQL site is running the chain — the one thing the two differ by.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GraphqlSite {
    /// A `#[query]` / `#[mutation]` / `#[subscription]`: the app-wide pool
    /// runs here, since `/graphql`'s edge is `EdgePosture::Exempt` and its
    /// operation guard is the app's authz bridge rather than the pool.
    Operation,
    /// A `#[field_resolver]`: its resolver's and method's guards run, the pool
    /// does not — the root field of the same request already ran it, and
    /// folding it again would run it once per parent.
    Field,
    /// An `#[entity]`, reached only through `_entities`, whose federation gate
    /// runs the pool once per field; folding it here would run it once per
    /// representation.
    Entity,
}

impl GraphqlSite {
    /// Whether this site composes the pool, read off the container so the memo
    /// cell's key still covers the whole composition.
    ///
    /// Skips only when a `FederationGate` is seeded: `GuardSpecs` can be seeded
    /// without one, and skipping then would leave the entity ungated.
    fn bucket(self) -> fn(&Container) -> GlobalBucket {
        match self {
            Self::Operation => |_| GlobalBucket::Fold,
            Self::Field => |_| GlobalBucket::Skip,
            Self::Entity => |container| match container.get::<FederationGate>() {
                Some(_) => GlobalBucket::Skip,
                None => GlobalBucket::Fold,
            },
        }
    }
}

/// GraphQL shaper helper. Called by `#[operations]` at the start of every
/// resolver method. Dedups per-resolver guards against the global chain.
///
/// `sources` is consulted only when the chain is composed. A `&dyn Fn` rather
/// than an `impl Fn`, so this body codegens once rather than per resolver.
/// GraphQL pipes run at the transport's request entry, not here.
pub async fn run_layered_graphql_chain(
    ctx: &GraphqlContext<'_>,
    container: &Container,
    cell: &SiteChainCell,
    route_label: &str,
    sources: &(dyn Fn() -> SiteChainSources + Sync),
    site: GraphqlSite,
) -> std::result::Result<(), GraphqlError> {
    run_chain(ctx, container, cell, route_label, sources, site).await
}

async fn run_chain(
    ctx: &GraphqlContext<'_>,
    container: &Container,
    cell: &SiteChainCell,
    route_label: &str,
    sources: &(dyn Fn() -> SiteChainSources + Sync),
    site: GraphqlSite,
) -> std::result::Result<(), GraphqlError> {
    let chain = cell.chain(container, route_label, sources, site.bucket());
    let operation = GraphqlOperationContext::field(ctx);
    for entry in chain.iter() {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        if let Err(denial) = entry.layer.as_ref().check_graphql(&operation).await {
            // Structural floor mirroring `deny_http`: every denial visible at
            // warn+ regardless of what the individual guard logged.
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
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
