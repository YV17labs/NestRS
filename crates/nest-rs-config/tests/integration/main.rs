//! Integration tests for the config crate's public macro surface.
//!
//! The whole binary runs under a **custom** env prefix: `env_prefix!` is a
//! link-time declaration, so a suite wanting both prefixes would need two
//! binaries, and the shipped default (`NESTRS`) is already covered by the unit
//! tests inside `src/`. Declaring `ACME` here is what makes the override an
//! executed contract rather than a documented intention.
nest_rs_core::env_prefix!("ACME");

mod diagnostics;
mod env_prefix;
