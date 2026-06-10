//! [`Strategy`] trait — how a request becomes an authenticated principal.

use async_trait::async_trait;
use poem::Request;

use crate::error::AuthError;

/// Turns a request into a principal. A strategy either authenticates the
/// caller (`Ok(principal)`) or reports why it could not (`Err`). A strategy
/// never issues a transport response itself — a redirect-style flow (OAuth
/// `/authorize`) is a plain handler, so authentication stays a pure
/// request → principal mapping.
#[async_trait]
pub trait Strategy: Send + Sync + 'static {
    type Principal: Clone + Send + Sync + 'static;

    async fn authenticate(&self, req: &mut Request) -> Result<Self::Principal, AuthError>;
}
