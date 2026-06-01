//! The identity **contract** shared between the apps — *business*-shared code, not
//! framework. The `auth` app **signs** these [`Claims`] into a JWT; the `api` app
//! (and any other resource server) **verifies** the token and reads them back. It
//! is the one type both sides must agree on, so it lives in a crate both depend on
//! rather than in either app. The verified `Claims` double as the resource server's
//! runtime principal. Keep this crate dependency-light — no framework, no transport.

mod claims;

pub use claims::{Claims, Role};
