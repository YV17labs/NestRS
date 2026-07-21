//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.

mod access_contract;
mod config;
mod cors;
mod exception_filters;
mod fail_secure_boot;
mod guards;
mod http;
mod interceptors;
mod keyed_providers;
mod layer_pool;
mod lifecycle_hooks;
mod pipes;
mod reflector;
mod request_scope;
mod strict_resolver_membership;
mod transient_scope;
mod transport_parity;
mod versioning_filters;
mod ws_gateway_guards;
