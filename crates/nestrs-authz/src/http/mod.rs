//! HTTP surface bindings for [`nestrs_authz`](crate). Enabled by the `http` Cargo feature.
//!
//! `nestrs-authz` is the transport-agnostic authorization engine; this module is
//! its poem binding, mirroring how `nestrs-http`'s `Valid`/`Piped` bind the pure
//! `nestrs-pipes`. Feature-gating it keeps `poem` and `sea-orm` (the masking
//! deserializes into `EntityTrait::Model`) out of the engine — each side keeps a
//! single responsibility.
//!
//! The pieces, in request order:
//! - [`AbilityGuard`] — the per-route guard that builds the request `Ability`
//!   from the actor an authentication guard attached.
//! - [`Authorize`] — the access gate (a poem extractor): `403` unless the
//!   ability grants action `A` on subject `S`.
//! - [`Scope`] — the caller's row-level filter as a `Condition` argument, for
//!   a handler that builds its own query.
//! - [`Authorize`]'s `RouteResponseShaper` impl (in `shape`) — `#[routes]` installs
//!   the ability as ambient state for the handler (so the data layer scopes reads)
//!   and masks the response to the fields and rows the ability permits, with no
//!   `mask` call in the handler.
//!
//! Route-model binding by id (the analog of these gates that *loads* the row)
//! lives in `nestrs_database::Bind` — it must `use` the data layer, so it sits
//! in `nestrs-database`'s `http` feature rather than here.

mod extractor;
mod guard;
mod scope;
mod shape;

pub use extractor::Authorize;
pub use guard::AbilityGuard;
pub use scope::Scope;
