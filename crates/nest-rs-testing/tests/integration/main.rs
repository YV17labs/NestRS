//! Integration tests organized by concern — the exception `CLAUDE.md`'s test-layout
//! norm grants this crate rather than mirroring `src/`. One binary, one module per
//! concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod access_contract;
mod config;
mod cors;
mod destructured_args;
mod env_cascade;
mod exception_filters;
mod fail_secure_boot;
mod guard_markers;
mod guards;
mod harness_parity;
mod http;
mod interceptors;
mod keyed_providers;
mod layer_pool;
mod lifecycle_hooks;
mod mcp;
mod pipes;
mod reflector;
mod request_scope;
mod shutdown;
mod transient_scope;
mod transport_parity;
mod versioning_filters;
mod ws_gateway_guards;
