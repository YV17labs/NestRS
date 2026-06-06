//! HTTP bindings for [`nest_rs_authz`](crate) (feature `http`).
//!
//! In request order: [`AbilityGuard`] builds the request `Ability` from the
//! actor an authn guard attached; [`Authorize`] gates access (`403` unless `A`
//! on `S` is granted); [`Scope`] hands a handler the row-level `Condition` to
//! build its own query; the `RouteResponseShaper` impl on [`Authorize`]
//! installs the ability as ambient state (data-layer scoping) and masks the
//! response — no `mask` call in the handler.
//!
//! By-id route-model binding lives in `nest_rs_seaorm::Bind` (it `use`s the
//! data layer).

mod extractor;
mod guard;
mod scope;
mod shape;

pub use extractor::Authorize;
pub use guard::AbilityGuard;
pub use scope::Scope;
