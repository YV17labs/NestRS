//! Postgres-backed tests for the database crate.
//!
//! Runs under `nestrs run test e2e`; gated out of `nestrs run test unit` by the
//! `binary(e2e)` nextest filter.
//!

mod harness;

mod create;
mod interceptor;
mod lazy;
mod relational_authz;
mod scope;
mod worker;
#[cfg(feature = "ws")]
mod ws;
