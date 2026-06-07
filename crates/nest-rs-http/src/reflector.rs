//! [`Reflector`] — read per-handler metadata a `#[meta(...)]` attribute
//! attached. Lets a guard read declarative route metadata by type (e.g. a
//! guard reads the route's required roles to vary its decision).
//!
//! Implements [`HandlerMetadata`] so a Layer written against the trait stays
//! portable across transports — the trait's [`is_public`] default reads the
//! attached [`Public`] marker uniformly.
//!
//! Binding constraint: bind the reading guard **per route** with
//! `#[use_guards]`. A *global* guard (`HttpTransport::guard`) runs before
//! routing resolves a handler, so route metadata is not yet attached and the
//! reflector finds nothing.

use std::any::Any;

use nest_rs_core::HandlerMetadata;
use poem::Request;

pub struct Reflector<'a>(&'a Request);

impl<'a> Reflector<'a> {
    pub fn new(req: &'a Request) -> Self {
        Reflector(req)
    }
}

impl<'a> HandlerMetadata for Reflector<'a> {
    fn get<M: Any + Send + Sync>(&self) -> Option<&M> {
        self.0.extensions().get::<M>()
    }
}
