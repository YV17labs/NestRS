//! Postgres-backed tests for the database crate.
//!
//! Runs under `just test-e2e`; gated out of `just test` by the
//! `binary(e2e)` nextest filter.
//!
//! The modules under `tests/e2e/` are pulled in via `#[path]` because Rust
//! resolves `mod foo;` siblings of `tests/e2e.rs` in `tests/`, not `tests/e2e/`.

#[path = "e2e/interceptor.rs"]
mod interceptor;
#[path = "e2e/scope.rs"]
mod scope;
#[path = "e2e/worker.rs"]
mod worker;
#[cfg(feature = "ws")]
#[path = "e2e/ws.rs"]
mod ws;
