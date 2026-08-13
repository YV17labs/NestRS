//! MCP per-operation chain runner. Called at the start of every `#[tool]` /
//! `#[prompt]` method by `#[tools]`.
//!
//! The cell, the sources and the composition live in [`chain`](super::chain);
//! this file is what MCP adds to them — reading the ambient app off the request
//! scope, `check_mcp`, and the JSON-RPC error a [`Denial`](crate::Denial)
//! renders as.
//!
//! # The pool runs here too, exactly as it does on `/graphql`
//!
//! `/mcp` is `EdgePosture::Exempt`, and its
//! [`McpOperationGuard`](nest_rs_mcp::McpOperationGuard) gates **every** request
//! at the endpoint — the app's authz bridge when one is registered, else
//! `FallbackMcpGuard` folding the global pool in-band, else deny-all. What that
//! endpoint runs is `check_http`, against the HTTP request the operation arrived
//! in; it has no operation to hand a `check_mcp`, and cannot acquire one.
//!
//! So the pool is part of *this* chain as well, and the two are not a
//! duplication: the edge asks "may this request reach the endpoint", this site
//! asks "may this caller run this operation". Excluding the pool here is what
//! left a global guard overriding only `check_mcp` never consulted — while its
//! mere presence disarmed the endpoint's deny-all tail, so registering it opened
//! what it was written to close.
//!
//! # Why the whole preamble is here
//!
//! Everything below — the ambient lookups, the context, the no-app fallback —
//! could have been emitted at each operation. Keeping it in one function means
//! the expansion at a `#[tool]` is a single call, so the branch is type-checked
//! and codegen'd once per crate instead of once per operation.

use nest_rs_mcp::{
    McpError, McpOperationContext, McpOperationKind, current_container, unresolvable_chain,
};

use crate::dispatch::chain::{GlobalBucket, SiteChainCell, SiteChainSources};
use crate::dispatch::denial_convert::denial_to_mcp_error;

/// MCP shaper helper. Called by `#[tools]` at the start of every decorated
/// operation.
///
/// `cell` memoizes the composed chain per app; `sources` is consulted only on
/// the miss that composes it, and is erased for the reason
/// [`run_layered_graphql_chain`](super::run_layered_graphql_chain) documents.
pub async fn run_layered_mcp_chain(
    cell: &SiteChainCell,
    route_label: &'static str,
    host: &'static str,
    kind: McpOperationKind,
    name: &'static str,
    sources: &(dyn Fn() -> SiteChainSources + Sync),
) -> Result<(), McpError> {
    let Some(container) = current_container() else {
        // No app is ambient — a mount assembled without the transport edge.
        // Guards that were *declared* cannot be resolved, so the operation fails
        // closed and named; one that declared none loses nothing. The pool is
        // not consulted here for the same reason: it lives in a container this
        // dispatch cannot reach.
        let declared = sources();
        if declared.provider.is_empty() && declared.method.is_empty() {
            return Ok(());
        }
        return Err(unresolvable_chain(route_label));
    };

    let chain = cell.chain(&container, route_label, sources, |_| GlobalBucket::Fold);
    if chain.is_empty() {
        return Ok(());
    }

    let ctx = McpOperationContext::new(&container, host, kind, name);
    for entry in chain.iter() {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        if let Err(denial) = entry.layer.as_ref().check_mcp(&ctx).await {
            // Structural floor mirroring `deny_http`: every denial visible at
            // warn+ regardless of what the individual guard logged. The host and
            // the operation are separate fields — an incident query asks "what
            // was refused on this tool", and a joined label cannot answer it.
            tracing::warn!(
                target: "nest_rs::layers",
                guard = entry.name,
                host = ctx.host(),
                kind = ctx.kind().as_str(),
                operation = ctx.name(),
                status = denial.http_status(),
                "guard denied the operation",
            );
            return Err(denial_to_mcp_error(denial));
        }
    }
    Ok(())
}
