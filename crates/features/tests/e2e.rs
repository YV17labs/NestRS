//! Postgres-backed tests for the features crate.
//!
//! Runs under `just test-e2e`; gated out of `just test` by the
//! `binary(e2e)` nextest filter.

#[path = "e2e/orgs/mod.rs"]
mod orgs;
