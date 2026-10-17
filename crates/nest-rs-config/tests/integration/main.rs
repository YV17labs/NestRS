//! Integration tests for the config crate's public macro surface.
//!
//! Needs nextest: each test freezes process-global state (the prefix, the
//! claim registry) and must own its process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

use std::process::Command;

mod dotenv;
mod env_prefix;
mod environment;
mod for_instance;
mod module;
mod namespace;
mod service;
mod unclaimed;

/// The variable a child reads its role from.
const CHILD_ROLE: &str = "NEST_RS_CONFIG_CHILD_ROLE";

/// This test binary, re-run on `test` alone, in the role [`is_child`] reads.
pub(crate) fn child(test: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args(["--exact", test, "--nocapture"])
        .env(CHILD_ROLE, "1");
    command
}

/// Whether a parent test started this process through [`child`].
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
pub(crate) fn is_child() -> bool {
    std::env::var(CHILD_ROLE).is_ok()
}
