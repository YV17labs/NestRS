//! `OpenTelemetryModule` must not be imported without `OpenTelemetry::init` first — that would
//! register no-op telemetry providers and drop traces/metrics silently, so the
//! boot fails instead. nextest runs every test in a process of its own, so a
//! test that initialises OpenTelemetry never does it for another.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod module;
