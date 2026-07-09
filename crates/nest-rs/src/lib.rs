//! `nestrs` — umbrella crate that re-exports the framework's surface so an
//! application can write a single `use nest_rs::prelude::*;` instead of a
//! handful of per-crate imports.
//!
//! The per-crate split (`nest-rs-core`, `nest-rs-http`, …) stays the public,
//! versioned source of truth. This crate adds no API of its own — it only
//! collects what already exists behind Cargo features, with one feature per
//! surface so an app pays only for what it uses.
pub use nest_rs_core as core;

#[cfg(feature = "http")]
pub use nest_rs_http as http;

#[cfg(feature = "config")]
pub use nest_rs_config as config;

#[cfg(feature = "database")]
pub use nest_rs_database as database;

#[cfg(feature = "seaorm")]
pub use nest_rs_seaorm as seaorm;

#[cfg(feature = "graphql")]
pub use nest_rs_graphql as graphql;

#[cfg(feature = "ws")]
pub use nest_rs_ws as ws;

#[cfg(feature = "mcp")]
pub use nest_rs_mcp as mcp;

#[cfg(feature = "queue")]
pub use nest_rs_queue as queue;

#[cfg(feature = "redis")]
pub use nest_rs_redis as redis;

#[cfg(feature = "schedule")]
pub use nest_rs_schedule as schedule;

#[cfg(feature = "events")]
pub use nest_rs_events as events;

#[cfg(feature = "authn")]
pub use nest_rs_authn as authn;

#[cfg(feature = "authz")]
pub use nest_rs_authz as authz;

#[cfg(feature = "opentelemetry")]
pub use nest_rs_opentelemetry as opentelemetry;

#[cfg(feature = "openapi")]
pub use nest_rs_openapi as openapi;

#[cfg(feature = "health")]
pub use nest_rs_health as health;

#[cfg(feature = "throttler")]
pub use nest_rs_throttler as throttler;

#[cfg(feature = "server-timing")]
pub use nest_rs_server_timing as server_timing;

#[cfg(feature = "testing")]
pub use nest_rs_testing as testing;

/// The everyday import — covers the decorators and types an app reaches for
/// on every controller, service, and module.
///
/// ```ignore
/// use nest_rs::prelude::*;
/// ```
///
/// Items behind Cargo features are pulled in only when the matching feature
/// is enabled. The default features (`http`, `config`) cover the typical
/// HTTP-API case.
pub mod prelude {
    pub use nest_rs_core::{
        App, AppBuilder, Container, ContainerBuilder, Module, hooks, injectable, module,
    };

    #[cfg(feature = "http")]
    pub use nest_rs_http::{
        ClientIp, Ctx, HttpConfig, HttpModule, RawBody, Reflector, Scoped, Valid, controller,
        http_code, input, redirect, response_header, routes,
    };

    #[cfg(feature = "http")]
    pub use nest_rs_http::poem::web::{Json, Path, Query};

    #[cfg(feature = "config")]
    pub use nest_rs_config::{Config, Namespaced, config};
}
