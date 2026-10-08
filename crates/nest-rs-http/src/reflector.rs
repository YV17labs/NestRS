//! [`Reflector`] — read per-handler metadata a `#[meta(...)]` attribute
//! attached.
//!
//! Every guard reads it, global or per route; a self-mounted endpoint
//! (`/graphql`, `/mcp`, a gateway) has no `#[meta]` site, so it finds nothing.

use std::any::Any;

use crate::metadata::HandlerMetadata;
use poem::Request;

/// Reads per-handler `#[meta(...)]` metadata off the live request by type.
pub struct Reflector<'a>(&'a Request);

impl<'a> Reflector<'a> {
    /// Wrap a request so a guard can read its attached route metadata.
    pub fn new(req: &'a Request) -> Self {
        Reflector(req)
    }
}

impl<'a> HandlerMetadata for Reflector<'a> {
    fn get<M: Any + Send + Sync>(&self) -> Option<&M> {
        self.0.extensions().get::<M>()
    }
}
