//! MCP per-operation chain runner. Called at the start of every `#[tool]` /
//! `#[prompt]` method by `#[tools]`.
//!
//! The pool is part of this chain, as on `/graphql`: the endpoint's
//! [`McpOperationGuard`](nest_rs_mcp::McpOperationGuard) runs `check_http`
//! against the request, never `check_mcp` against an operation, so leaving the
//! pool out here would leave a global guard overriding only `check_mcp` never
//! consulted.

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
        // No app is ambient: declared guards cannot be resolved, so the
        // operation fails closed; one that declared none loses nothing.
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
            // Structural floor mirroring `deny_http`; host and operation stay
            // separate fields for an incident query.
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
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
