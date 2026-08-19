//! [`Reflector`] — read per-handler metadata a `#[meta(...)]` attribute
//! attached. Lets a guard read declarative route metadata by type (e.g. a
//! guard reads the route's required roles to vary its decision).
//!
//! Implements [`HandlerMetadata`] so a Layer written against the trait stays
//! portable across transports — the trait's [`is_public`] default reads the
//! attached [`Public`] marker uniformly.
//!
//! Scope does not change what a guard reads. `#[routes]` attaches the metadata
//! as route data *outside* the guard chain, so it is on the request by the time
//! any guard runs — pooled globally with `use_guards_global` or bound per route
//! with `#[use_guards]`, both read it here.
//!
//! A self-mounted endpoint (`/graphql`, `/mcp`, a gateway) has no route data to
//! read: there is no `#[meta]` site there, and the reflector finds nothing.

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
