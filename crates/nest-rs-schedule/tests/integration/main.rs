//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod diagnostics;
mod error;
mod module;
mod occurrence;
mod scheduler;

/// A container whose reachable set is seeded empty, so a scheduler configured
/// against it starts the jobs its test attaches and nothing else.
///
/// `configure` also walks the link-time `ScheduledMethod` registry, and with no
/// gate seeded it starts every `#[scheduled]` method compiled into this binary:
/// the ticks of another module's fixtures land in a test's log capture and its
/// lock's records, and one of them declares `replicas = "one"`, which fails the
/// boot of any test that binds no lock. Empty and *present* is what gates them.
pub(crate) fn hermetic() -> nest_rs_core::ContainerBuilder {
    nest_rs_core::Container::builder().provide(nest_rs_core::ReachableProviders(Default::default()))
}
