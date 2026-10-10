//! `nestrs` — umbrella crate that re-exports the framework's surface so an
//! application can write a single `use nest_rs::prelude::*;` instead of a
//! handful of per-crate imports. Each surface sits behind its own Cargo
//! feature, so an app pays only for what it uses.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

pub use nest_rs_core as core;

/// The binary's entry point: `#[nest_rs::main] async fn main()`. It builds the
/// runtime, runs the app on it, and ends the process within the shutdown
/// budget — see [`core::main`].
pub use nest_rs_core::main;

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

/// RFC 6749 §1.1's roles, one module per role, spelled in the RFC's own words.
#[cfg(any(
    feature = "oauth-client",
    feature = "oauth-server",
    feature = "oauth-resource"
))]
pub mod oauth {
    #[cfg(feature = "oauth-client")]
    pub use nest_rs_oauth_client as client;
    #[cfg(feature = "oauth-resource")]
    pub use nest_rs_oauth_resource as resource;
    #[cfg(feature = "oauth-server")]
    pub use nest_rs_oauth_server as server;
}
#[cfg(feature = "social")]
pub use nest_rs_social as social;

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

#[cfg(feature = "static-files")]
pub use nest_rs_static_files as static_files;

#[cfg(feature = "testing")]
pub use nest_rs_testing as testing;

// Layer-System extension-point crates, so a custom `Guard`/`Pipe`/`Interceptor`/
// `Filter`/`ExceptionFilter` needs no per-crate dependency.
#[cfg(feature = "guards")]
pub use nest_rs_guards as guards;

#[cfg(feature = "pipes")]
pub use nest_rs_pipes as pipes;

#[cfg(feature = "interceptors")]
pub use nest_rs_interceptors as interceptors;

#[cfg(feature = "filters")]
pub use nest_rs_filters as filters;

#[cfg(feature = "exception-filters")]
pub use nest_rs_exception_filters as exception_filters;

#[cfg(feature = "storage")]
pub use nest_rs_storage as storage;

#[cfg(feature = "worker")]
pub use nest_rs_worker as worker;

/// The everyday import — covers the decorators and types an app reaches for
/// on every controller, service, and module.
///
/// ```
/// use nest_rs::prelude::*;
/// # fn everyday(_: App, _: AppBuilder, _: Container, _: ContainerBuilder) {}
/// # fn root<M: Module>() {}
/// ```
///
/// Items behind Cargo features are pulled in only when the matching feature
/// is enabled. The default features (`http`, `config`) cover the typical
/// HTTP-API case.
pub mod prelude {
    pub use nest_rs_core::{
        App, AppBuilder, Container, ContainerBuilder, Module, hooks, injectable, module,
    };

    // Outside every edge's feature: a headless worker needs `#[input]` too.
    pub use nest_rs_core::input;

    #[cfg(feature = "http")]
    pub use nest_rs_http::{
        ClientIp, Ctx, HttpConfig, HttpModule, RawBody, Reflector, Scoped, Valid, controller,
        http_code, interceptor, redirect, response_header, routes,
    };

    // poem's extractors on purpose: poem's major is tied to nestrs's (root `Cargo.toml`).
    #[cfg(feature = "http")]
    pub use nest_rs_http::poem::web::{Json, Path, Query};

    #[cfg(feature = "config")]
    pub use nest_rs_config::{Config, Namespaced, config};

    #[cfg(feature = "events")]
    pub use nest_rs_events::listeners;

    #[cfg(feature = "graphql")]
    pub use nest_rs_graphql::{dataloader, operations, resolver};

    #[cfg(feature = "health")]
    pub use nest_rs_health::indicators;

    #[cfg(feature = "mcp")]
    pub use nest_rs_mcp::{mcp, tools};

    #[cfg(feature = "queue")]
    pub use nest_rs_queue::{processor, queue};

    #[cfg(feature = "seaorm")]
    pub use nest_rs_seaorm::{expose, wire_enum};

    #[cfg(feature = "schedule")]
    pub use nest_rs_schedule::scheduled;

    #[cfg(feature = "ws")]
    pub use nest_rs_ws::{gateway, messages};

    // No `#[crud]`: the HTTP and GraphQL decorators share the name, so a glob
    // would collide; import it from its edge (`nest_rs::graphql::crud`).
}
