//! `nestrs` — umbrella crate that re-exports the framework's surface so an
//! application can write a single `use nestrs::prelude::*;` instead of a
//! handful of per-crate imports.
//!
//! The per-crate split (`nestrs-core`, `nestrs-http`, …) stays the public,
//! versioned source of truth. This crate adds no API of its own — it only
//! collects what already exists behind Cargo features, with one feature per
//! surface so an app pays only for what it uses.

pub use nestrs_core as core;

#[cfg(feature = "http")]
pub use nestrs_http as http;

#[cfg(feature = "config")]
pub use nestrs_config as config;

#[cfg(feature = "database")]
pub use nestrs_database as database;

#[cfg(feature = "seaorm")]
pub use nestrs_seaorm as seaorm;

#[cfg(feature = "graphql")]
pub use nestrs_graphql as graphql;

#[cfg(feature = "ws")]
pub use nestrs_ws as ws;

#[cfg(feature = "mcp")]
pub use nestrs_mcp as mcp;

#[cfg(feature = "queue")]
pub use nestrs_queue as queue;

#[cfg(feature = "redis")]
pub use nestrs_redis as redis;

#[cfg(feature = "schedule")]
pub use nestrs_schedule as schedule;

#[cfg(feature = "events")]
pub use nestrs_events as events;

#[cfg(feature = "authn")]
pub use nestrs_authn as authn;

#[cfg(feature = "authz")]
pub use nestrs_authz as authz;

#[cfg(feature = "opentelemetry")]
pub use nestrs_opentelemetry as opentelemetry;

#[cfg(feature = "openapi")]
pub use nestrs_openapi as openapi;

#[cfg(feature = "health")]
pub use nestrs_health as health;

#[cfg(feature = "throttler")]
pub use nestrs_throttler as throttler;

#[cfg(feature = "server-timing")]
pub use nestrs_server_timing as server_timing;

#[cfg(feature = "testing")]
pub use nestrs_testing as testing;

/// The everyday import — covers the decorators and types an app reaches for
/// on every controller, service, and module.
///
/// ```ignore
/// use nestrs::prelude::*;
/// ```
///
/// Items behind Cargo features are pulled in only when the matching feature
/// is enabled. The default features (`http`, `config`) cover the typical
/// HTTP-API case.
pub mod prelude {
    pub use nestrs_core::{App, AppBuilder, Container, ContainerBuilder, Module, hooks, injectable, module};

    #[cfg(feature = "http")]
    pub use nestrs_http::{
        ClientIp, Ctx, HttpConfig, HttpModule, RawBody, Reflector, Scoped, Valid, controller,
        http_code, input, redirect, response_header, routes,
    };

    #[cfg(feature = "http")]
    pub use nestrs_http::poem::web::{Json, Path, Query};

    #[cfg(feature = "config")]
    pub use nestrs_config::{Config, Namespaced, config};
}
