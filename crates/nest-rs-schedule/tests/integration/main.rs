//! Integration tests mirroring `src/` — one binary, one module per concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod error;
mod module;
mod occurrence;
mod scheduler;

/// A container whose reachable set is seeded empty, so a scheduler configured
/// against it starts the jobs its test attaches and nothing else: with no gate
/// seeded it would start every `#[scheduled]` fixture compiled into this binary.
pub(crate) fn hermetic() -> nest_rs_core::ContainerBuilder {
    nest_rs_core::Container::builder().provide(nest_rs_core::ReachableProviders(Default::default()))
}
