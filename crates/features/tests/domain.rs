//! Pure-domain integration tests — no Postgres required.
//!
//! Documented gaps: `authn/` and `authz/` DI modules; `oauth/strategy.rs`.
//! DB-backed cases (currently `orgs::service`) live in the `e2e` binary
//! alongside this crate's `tests/e2e.rs`.
//! See each submodule's `mod.rs` for where behaviour is exercised instead.

mod authn;
mod authz;
mod oauth;
mod users;
