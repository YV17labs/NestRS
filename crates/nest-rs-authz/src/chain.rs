//! The authn → authz ordering, written once, for the per-operation bridges
//! whose transport is `EdgePosture::Exempt` at the HTTP edge (GraphQL, MCP).

use nest_rs_guards::{Denial, Guard};
use poem::Request;

/// Authenticate, then authorize. Short-circuits on the first denial, which is
/// returned **as the guard raised it** — the caller maps it, so a throttler's
/// `429` or an authn `401` keeps its status instead of being flattened.
pub async fn run_ability_chain(
    auth: &dyn Guard,
    ability: &dyn Guard,
    req: &mut Request,
) -> Result<(), Denial> {
    auth.check_http(req).await?;
    ability.check_http(req).await
}
