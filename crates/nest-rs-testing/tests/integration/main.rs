//! Integration tests organized by concern — the exception `CLAUDE.md`'s test-layout
//! norm grants this crate rather than mirroring `src/`. One binary, one module per
//! concern.

mod access_contract;
mod config;
mod cors;
mod destructured_args;
mod env_cascade;
mod exception_filters;
mod fail_secure_boot;
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
mod transient_scope;
mod transport_parity;
mod versioning_filters;
mod ws_gateway_guards;
