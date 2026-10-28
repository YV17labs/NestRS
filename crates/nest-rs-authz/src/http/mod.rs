//! HTTP bindings for [`nest_rs_authz`](crate) (feature `http`).
//!
//! In request order: [`AbilityGuard`](crate::AbilityGuard) builds the request
//! `Ability` from the actor an authn guard attached; [`Authorize`] gates access
//! (`403` unless `A` on `S` is granted); [`Scope`] hands a handler the
//! row-level `Condition` to build its own query; the `RouteResponseShaper` impl
//! on [`Authorize`] installs the ability as ambient state (data-layer scoping)
//! and masks the response — no `mask` call in the handler.
//!
//! A route loads its subject by id with `#[authorize(A, bind = S)]`, whose
//! loader lives in `nest-rs-seaorm`.

mod authorize;
pub(crate) mod mask;
mod scope;

pub use authorize::Authorize;
pub use mask::mask_entity_response;
pub use scope::Scope;
