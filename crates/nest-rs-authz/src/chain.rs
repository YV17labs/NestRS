//! The authn → authz ordering, written once, for the per-operation bridges
//! of the endpoints that gate in band (GraphQL, MCP).

use nest_rs_core::Handler;
use nest_rs_guards::{Denial, Guard};
use poem::Request;

/// Authenticate, then authorize, on `req` as the unit `endpoint` declares,
/// keeping what the two attach on the request for the endpoint to run its
/// operations under. Short-circuits on the first denial, which is returned
/// **as the guard raised it** — the caller maps it, so a throttler's `429` or
/// an authn `401` keeps its status instead of being flattened.
pub(crate) async fn run_ability_chain(
    endpoint: &'static Handler,
    auth: &dyn Guard,
    ability: &dyn Guard,
    req: &mut Request,
) -> Result<(), Denial> {
    nest_rs_http::__private::check_request(
        endpoint,
        [("authentication", auth), ("authorization", ability)],
        req,
    )
    .await
}
