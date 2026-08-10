//! GraphQL per-site chain runner. Emitted inline at the start of every
//! `#[query]` / `#[mutation]` / `#[field_resolver]` by `#[operations]`.
//!
//! The cell, the sources and the composition live in [`chain`](super::chain);
//! this file is what GraphQL adds to them — `check_graphql` and the error frame
//! a [`Denial`](crate::Denial) renders as.

use nest_rs_core::Container;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};

use crate::dispatch::chain::{GlobalScope, SiteChainCell, SiteChainSources};
use crate::dispatch::denial_convert::denial_to_graphql_error;

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
/// The app-wide pool **runs** here ([`GlobalScope::AtThisSite`]): `/graphql`'s
/// edge is `EdgePosture::Exempt` and its operation guard is the app's authz
/// bridge, not the pool, so the resolver site is where a pooled guard reaches an
/// operation.
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
) -> std::result::Result<(), GraphqlError> {
    let chain = cell.chain(
        container,
        route_label,
        GlobalScope::AtThisSite,
        &[],
        sources,
    );
    for entry in chain.iter() {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        if let Err(denial) = entry.layer.as_ref().check_graphql(ctx).await {
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
