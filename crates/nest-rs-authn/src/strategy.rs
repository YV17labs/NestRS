//! [`Strategy`] trait — how a request becomes an authenticated principal.

use std::time::Duration;

use async_trait::async_trait;
use poem::Request;

use crate::PrincipalIdentity;
use crate::error::AuthError;

/// How long [`AuthnGuard`](crate::AuthnGuard) waits for
/// [`Strategy::authenticate`] to answer before it denies the request.
///
/// Past it the request fails closed on every route, `#[public]` included, as
/// for an unreachable identity store. It sits above the longest shipped login
/// (GitHub social login: three `OAuthClient` calls bounded at 5 s each) and
/// below the HTTP edge's request timeout (`<PREFIX>_HTTP__REQUEST_TIMEOUT_SECS`,
/// 20 s by default), which counts from the request's arrival and would
/// otherwise answer first, naming no strategy.
pub const AUTHENTICATE_TIMEOUT: Duration = Duration::from_secs(18);

/// Turns a request into a principal. A strategy either authenticates the
/// caller (`Ok(principal)`) or reports why it could not (`Err`). A strategy
/// never issues a transport response itself — a redirect-style flow (OAuth
/// `/authorize`) is a plain handler, so authentication stays a pure
/// request → principal mapping.
///
/// **`authenticate` answers within [`AUTHENTICATE_TIMEOUT`], or is denied.** A
/// strategy calling a backend bounds each call itself, and keeps no state a call
/// dropped at the bound would have had to undo.
///
/// **On failure**, the guard decides, and it fails closed: a strategy still
/// silent at the bound, or one that panics, decided nothing, so the request is
/// denied `503` on every route, `#[public]` included, as for
/// [`AuthError::Unavailable`] — said once by the guard, at `warn`, or at
/// `error` for the panic.
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
