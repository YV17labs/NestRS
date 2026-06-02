//! What counts as an authorization *subject*, and the bridge that makes the
//! ORM's entities qualify.
//!
//! Isolating the bridge here keeps the route-facing gate ([`Authorize`](crate::http::Authorize))
//! and the action vocabulary free of any ORM type: a handler names a subject as
//! `Authorize<Read, users::Entity>`, bounded only by [`Subject`]. The ORM
//! coupling lives entirely in the lowering surface (`predicate`, `ability`,
//! `builder`) plus this one bridge, so introducing a second ORM — or extracting
//! a `nestrs-authz-<orm>` adapter crate — moves a contained set of impls and
//! leaves the rest of the engine untouched.

/// A type the rules and routes refer to as a subject. Implemented for every
/// SeaORM entity by the blanket bridge below; an app names one as the `S` in
/// [`Authorize<A, S>`](crate::http::Authorize). The bound is a compile-time guardrail
/// that `S` is a real subject rather than an arbitrary type.
pub trait Subject: 'static {}

// The ORM bridge: every SeaORM entity is a subject. When a second ORM appears
// (behind a `nestrs-authz-<orm>` adapter), this is the single impl that moves.
impl<E: sea_orm::EntityTrait> Subject for E {}
