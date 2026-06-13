//! [`Strategy`] trait — how a request becomes an authenticated principal.

use async_trait::async_trait;
use poem::Request;

use crate::error::AuthError;
use crate::passport::PrincipalIdentity;

/// Turns a request into a principal. A strategy either authenticates the
/// caller (`Ok(principal)`) or reports why it could not (`Err`). A strategy
/// never issues a transport response itself — a redirect-style flow (OAuth
/// `/authorize`) is a plain handler, so authentication stays a pure
/// request → principal mapping.
#[async_trait]
pub trait Strategy: Send + Sync + 'static {
    /// The authenticated identity. Its [`PrincipalIdentity`] bound is what
    /// lets the framework record `actor_id` on the request span on success
    /// — every principal must say who it is for audit (or `None`).
    type Principal: PrincipalIdentity + Clone + Send + Sync + 'static;

    async fn authenticate(&self, req: &mut Request) -> Result<Self::Principal, AuthError>;
}
