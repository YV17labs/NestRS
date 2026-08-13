//! GraphQL per-site chain runner. Emitted inline at the start of every
//! `#[query]` / `#[mutation]` / `#[entity]` / `#[field_resolver]` by
//! `#[operations]`, which names the site — `#[entity]` leaves the app-wide pool
//! to the federation gate, everything else folds it.
//!
//! The cell, the sources and the composition live in [`chain`](super::chain);
//! this file is what GraphQL adds to them — `check_graphql` and the error frame
//! a [`Denial`](crate::Denial) renders as.

use nest_rs_core::Container;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};
use nest_rs_graphql::{FederationGate, GraphqlOperationContext};

use crate::dispatch::chain::{GlobalBucket, SiteChainCell, SiteChainSources};
use crate::dispatch::denial_convert::denial_to_graphql_error;

/// Which GraphQL site is running the chain — the one thing the two differ by.
///
/// A parameter rather than a second public runner: `#[operations]` emits a
/// literal instead of choosing an identifier, `nest-rs-guards` publishes one
/// seam, and the third site that ever needs its own treatment adds a variant
/// here rather than a third `pub fn`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GraphqlSite {
    /// A `#[query]` / `#[mutation]` / `#[field_resolver]`: the app-wide pool
    /// runs here, since `/graphql`'s edge is `EdgePosture::Exempt` and its
    /// operation guard is the app's authz bridge rather than the pool.
    Operation,
    /// An `#[entity]`, reached only through `_entities` — in front of which
    /// `nest_rs_graphql`'s federation gate runs the pool once per field,
    /// whatever the representation count. Folding it again here would run every
    /// pooled guard once per representation, against a context that cannot
    /// differ between them: the *exactly once* invariant broken in the one place
    /// a router controls the multiplier.
    Entity,
}

impl GraphqlSite {
    /// Whether this site composes the pool, read off the container so the memo
    /// cell's key still covers the whole composition.
    ///
    /// **Subtract only what the gate is actually there to run.** The gate is
    /// installed if and only if the app seeded a `FederationGate`, and only
    /// `use_guards_global` seeds one — while `GuardSpecs`, the pool itself, is a
    /// plain public provider an app can seed on its own. Skipping
    /// unconditionally turned *that* composition from gated into open.
    ///
    /// The gate is also installed only under `GraphqlConfig::federation`, and
    /// that costs nothing here: an `#[entity]` cannot exist without the flag —
    /// the boot refuses the pair — so an entity body only ever runs in a schema
    /// where the extension was installed.
    fn bucket(self) -> fn(&Container) -> GlobalBucket {
        match self {
            Self::Operation => |_| GlobalBucket::Fold,
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
/// `cell` memoizes the composed chain for `container`; `sources` is consulted
/// only when it has to be composed (see the module docs). It is a `&dyn Fn`
/// rather than an `impl Fn` on purpose: the closure is a distinct ZST per
/// resolver, so a generic parameter would monomorphize this whole body — the
/// future, the `tracing` callsite and all — once per operation in the app,
/// where the erased form codegens once per crate.
///
/// GraphQL pipes ([`nest_rs_pipes::GlobalPipe::transform_graphql_variables`])
/// are not invoked here — variables live at the operation level, not per
/// resolver, so they run at the GraphQL transport's request entry
/// (`nest_rs_graphql::context` folds them over an operation's variables).
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
