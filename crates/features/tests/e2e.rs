//! Postgres-backed tests for the features crate.
//!
//! Runs under `nestrs run test e2e`; gated out of `nestrs run test unit` by the
//! `binary(e2e)` nextest filter.

#[path = "e2e/orgs/mod.rs"]
mod orgs;

#[path = "e2e/posts_http.rs"]
mod posts_http;
