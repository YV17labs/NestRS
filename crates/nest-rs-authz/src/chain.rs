//! The authn → authz ordering, written once.
//!
//! Both per-operation bridges ([`graphql::GraphqlAbilityBridge`](crate::graphql),
//! [`mcp::McpAbilityBridge`](crate::mcp)) run the controllers' guard chain
//! themselves, because their transport is `EdgePosture::Exempt` at the HTTP
//! edge. Only the *error shape* differs between them, so the ordering lives
//! here and each bridge maps the returned [`Denial`] into its own transport
//! error. A fix to the chain — an added leg, a changed short-circuit — then
//! reaches every transport by construction.

use nest_rs_guards::{Denial, Guard};
use poem::Request;

/// Authenticate, then authorize. Short-circuits on the first denial, which is
/// returned **as the guard raised it** — the caller maps it, so a throttler's
/// `429` or an authn `401` keeps its status instead of being flattened.
///
/// Each guard logs its own denial at the source layer (`AuthnGuard` under
/// `nest_rs::authn`, `AbilityGuard` under `nest_rs::authz`), so nothing is
/// logged here: the event is said once, at its source.
pub async fn run_ability_chain(
    auth: &dyn Guard,
    ability: &dyn Guard,
    req: &mut Request,
) -> Result<(), Denial> {
    auth.check_http(req).await?;
    ability.check_http(req).await
}
