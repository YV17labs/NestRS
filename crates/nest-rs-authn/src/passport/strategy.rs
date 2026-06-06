//! [`Strategy`] trait and [`Outcome`] — how a request becomes an authenticated principal.

use async_trait::async_trait;
use poem::{Request, Response};

use crate::error::AuthError;

/// A strategy either authenticates or challenges the client (redirect / 401).
pub enum Outcome<P> {
    Authenticated(P),
    Challenge(Response),
}

#[async_trait]
pub trait Strategy: Send + Sync + 'static {
    type Principal: Clone + Send + Sync + 'static;

    async fn authenticate(&self, req: &mut Request) -> Result<Outcome<Self::Principal>, AuthError>;
}
