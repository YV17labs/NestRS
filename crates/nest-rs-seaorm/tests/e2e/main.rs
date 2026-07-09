//! Postgres-backed tests for the database crate.
//!
//! Runs under `nestrs run test e2e`; gated out of `nestrs run test unit` by the
//! `binary(e2e)` nextest filter.
//!

mod interceptor;
mod scope;
mod worker;
#[cfg(feature = "ws")]
mod ws;
