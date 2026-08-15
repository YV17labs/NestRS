//! Integration tests for the config crate's public macro surface.
//!
//! The custom-prefix tests set `NESTRS_ENV_PREFIX` and read it back in the same
//! process, which nextest makes honest: it runs every test in its own process,
//! so the `OnceLock` each one freezes is its own. Bare `cargo test` would share
//! one process between them and is unsupported for exactly this class of reason.

mod diagnostics;
mod dotenv;
mod env_prefix;
