//! [`Reflector`] — read declarative per-handler metadata attached with
//! `#[meta(...)]`. The NestJS `Reflector` analog: a guard asks for a metadata
//! value *by type* to vary its decision (the `@Roles` pattern — a guard reads
//! the route's required roles and compares them to the caller's).
//!
//! `#[routes]` evaluates each `#[meta(EXPR)]` once at mount and inserts the value
//! into the request just *outside* the route's `#[use_guards]` guards, so a
//! per-route guard reads it back here before deciding. Two consequences:
//!
//! - Bind the guard **per route** (`#[use_guards]`). A *global* guard
//!   (`HttpTransport::guard`) runs before routing resolves a handler, so route
//!   metadata is not yet attached and the reflector finds nothing.
//! - Metadata is keyed by type, not a string (the project's typed-over-stringly
//!   posture): declare a dedicated type per concern (`RequiredRoles`, `Public`)
//!   and read it back by that type.

use poem::Request;

/// Reads route metadata that `#[meta(...)]` attached, from the request a guard
/// is checking. Construct it with [`new`](Reflector::new) from the guard's
/// `&mut Request` and call [`get`](Reflector::get).
pub struct Reflector<'a>(&'a Request);

impl<'a> Reflector<'a> {
    /// Wrap the request a guard received (a `&mut Request` coerces here).
    pub fn new(req: &'a Request) -> Self {
        Reflector(req)
    }

    /// The metadata value of type `T` the matched route declared via
    /// `#[meta(...)]`, or `None` if it declared none of that type.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.extensions().get::<T>()
    }
}
