//! [`Strategy`] trait — how a request becomes an authenticated principal.

use std::time::Duration;

use async_trait::async_trait;
use poem::Request;

use crate::PrincipalIdentity;
use crate::error::AuthError;

/// How long [`AuthnGuard`](crate::AuthnGuard) waits for
/// [`Strategy::authenticate`] to answer before it denies the request.
///
/// A strategy that never answers held its request for as long, and nothing about
/// the credential was ever decided: the caller could be neither admitted nor
/// refused. Past this bound the guard gives the answer an unreachable identity
/// store gets — the credential was not evaluated, so the request fails closed on
/// every route, `#[public]` included, and is never let through as anonymous —
/// and says so at `warn`, naming the strategy.
///
/// The value sits between the two bounds either side of it, with room on both:
///
/// - **Above every strategy the framework ships.** The JWT strategy verifies
///   locally and answers on its first poll. The one login that crosses a network
///   is social login, whose provider calls go through `nest-rs-oauth-client`'s
///   `OAuthClient`, each bounded at `OAuthClient::CALL_TIMEOUT` (5 s); the
///   longest provider shipped, GitHub, makes three of them one after another, so
///   15 s. The guard must not pre-empt any of them, or it would cut short a
///   login still answering and replace the client's named cause — which
///   endpoint, which bound — with a bare timeout. What stays above 15 s is the
///   strategy's own work: resolving the identity it was handed.
/// - **Below the HTTP edge's own request timeout**
///   (`<PREFIX>_HTTP__REQUEST_TIMEOUT_SECS`, 30 s by default), which answers `503`
///   with a line naming no strategy. Every edge authenticates the HTTP request it
///   begins with — a route, the GraphQL POST, the MCP POST, the WebSocket
///   upgrade — so that timeout covers all of them; staying under it makes a hung
///   strategy read the same on every edge, the guard's denial and the guard's
///   line, and leaves this net the only bound where a deployment switched the
///   edge's off.
///
/// A constant rather than a setting: a strategy that answers at all answers well
/// inside it, and one that waits on a backend bounds that wait itself.
pub const AUTHENTICATE_TIMEOUT: Duration = Duration::from_secs(20);

/// Turns a request into a principal. A strategy either authenticates the
/// caller (`Ok(principal)`) or reports why it could not (`Err`). A strategy
/// never issues a transport response itself — a redirect-style flow (OAuth
/// `/authorize`) is a plain handler, so authentication stays a pure
/// request → principal mapping.
///
/// **`authenticate` answers within [`AUTHENTICATE_TIMEOUT`], or is treated as
/// unable to.** The guard waits no longer, then denies. That is the guard's net,
/// not a strategy's budget: a strategy that calls a backend bounds each call it
/// makes, as `OAuthClient` does, so an outage reaches the caller as the
/// strategy's own failure, with its cause, and promptly. And since a call is
/// dropped where it stands when the bound passes, a strategy keeps no state a
/// dropped call would have had to undo.
#[async_trait]
pub trait Strategy: Send + Sync + 'static {
    /// The authenticated identity. Its [`PrincipalIdentity`] bound is what
    /// lets the framework record `actor_id` on the request span on success
    /// — every principal must say who it is for audit (or `None`).
    type Principal: PrincipalIdentity + Clone + Send + Sync + 'static;

    /// Map the request to a principal, or return why authentication failed.
    /// Must not issue a transport response — a pure request → principal step.
    async fn authenticate(&self, req: &mut Request) -> Result<Self::Principal, AuthError>;
}
