//! [`Reflector`] — read per-handler metadata a `#[meta(...)]` attribute
//! attached. Lets a guard read declarative route metadata by type (e.g. a
//! guard reads the route's required roles to vary its decision).
//!
//! Binding constraint: bind the reading guard **per route** with
//! `#[use_guards]`. A *global* guard (`HttpTransport::guard`) runs before
//! routing resolves a handler, so route metadata is not yet attached and the
//! reflector finds nothing.

use poem::Request;

pub struct Reflector<'a>(&'a Request);

impl<'a> Reflector<'a> {
    pub fn new(req: &'a Request) -> Self {
        Reflector(req)
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.extensions().get::<T>()
    }
}
