//! HTTP bindings for [`nest_rs_authz`](crate) (feature `http`).
//!
//! In request order: [`AbilityGuard`](crate::AbilityGuard) builds the request
//! `Ability` from the actor an authn guard attached; the [`Authorize`] a route's
//! `#[authorize(A, S)]` or `#[public(A, S)]` declares gates access (`403` unless `A`
//! on `S` is granted) and opens the route's reads to that ability; each wire row
//! the handler answers was judged where `#[expose]`'s conversion built it, so the
//! handler checks nothing itself.
//!
//! A route loads its subject by id with `#[authorize(A, bind = S)]`, whose
//! loader lives in `nest-rs-seaorm`.

mod authorize;

pub use authorize::Authorize;
