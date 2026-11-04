//! `nest-rs` — the umbrella that re-exports the framework's surface so an
//! application can write a single `use nest_rs::prelude::*;` instead of a
//! handful of per-crate imports. Each surface sits behind its own Cargo
//! feature, so an app pays only for what it uses.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

// A module over a glob, never `pub use nest_rs_core as core`: rustc names a crate
// re-exported under another name by its own, `nest_rs::nest_rs_core::…`, in every
// diagnostic, a path no app can write. Each is written out: a module a
// `macro_rules!` expands loses to `prelude` in the path rustc prints
// (`nest_rs::prelude::Json`).

/// The DI container, modules, the lifecycle and the unit pipeline every edge runs through.
pub mod core {
    #[doc(no_inline)]
    pub use nest_rs_core::*;
}

/// The binary's entry point: `#[nest_rs::main] async fn main()`. It builds the
/// runtime, runs the app on it, and ends the process within the shutdown
/// budget. Its `SIGINT` / `SIGTERM` handlers are installed before `main` runs,
/// so the first signal stops the boot or drains the app, and one received
/// before any app exists exits at once with 130 / 143 — see [`core::main`].
pub use nest_rs_core::main;

/// The HTTP edge: controllers, routes, the request vocabulary and the server.
#[cfg(feature = "http")]
pub mod http {
    #[doc(no_inline)]
    pub use nest_rs_http::*;
}

/// Typed configuration, read from the environment under each config's namespace.
#[cfg(feature = "config")]
pub mod config {
    #[doc(no_inline)]
    pub use nest_rs_config::*;
}

/// The data-layer port: the unit of work every edge runs through.
#[cfg(feature = "database")]
pub mod database {
    #[doc(no_inline)]
    pub use nest_rs_database::*;
}

/// SeaORM: the pool, the database binding, `Repo`, `#[expose]` and `#[crud]`'s services.
#[cfg(feature = "seaorm")]
pub mod seaorm {
    #[doc(no_inline)]
    pub use nest_rs_seaorm::*;
}

/// The GraphQL edge: resolvers, operations and the schema, on async-graphql.
#[cfg(feature = "graphql")]
pub mod graphql {
    #[doc(no_inline)]
    pub use nest_rs_graphql::*;
}

/// The WebSocket edge: gateways and their messages.
#[cfg(feature = "ws")]
pub mod ws {
    #[doc(no_inline)]
    pub use nest_rs_ws::*;
}

/// The Model Context Protocol edge: hosts, tools, prompts and resources, on rmcp.
#[cfg(feature = "mcp")]
pub mod mcp {
    #[doc(no_inline)]
    pub use nest_rs_mcp::*;
}

/// The queue port: queues, jobs and the `#[processor]` that drains them.
#[cfg(feature = "queue")]
pub mod queue {
    #[doc(no_inline)]
    pub use nest_rs_queue::*;
}

/// Redis: the connection, and the queue, throttler and schedule bindings over it.
#[cfg(feature = "redis")]
pub mod redis {
    #[doc(no_inline)]
    pub use nest_rs_redis::*;
}

/// Scheduled jobs: `#[scheduled]` with `#[every]`, `#[cron]` and `#[after]`.
#[cfg(feature = "schedule")]
pub mod schedule {
    #[doc(no_inline)]
    pub use nest_rs_schedule::*;
}

/// The in-process event bus and its `#[listeners]`.
#[cfg(feature = "events")]
pub mod events {
    #[doc(no_inline)]
    pub use nest_rs_events::*;
}

/// Authentication: `JwtService`, the `Strategy` port and `AuthnGuard`.
#[cfg(feature = "authn")]
pub mod authn {
    #[doc(no_inline)]
    pub use nest_rs_authn::*;
}

/// Authorization: abilities, `#[authorize]`, the query filter and field masking.
#[cfg(feature = "authz")]
pub mod authz {
    #[doc(no_inline)]
    pub use nest_rs_authz::*;
}

/// RFC 6749 §1.1's roles, one module per role, spelled in the RFC's own words.
#[cfg(any(
    feature = "oauth-client",
    feature = "oauth-server",
    feature = "oauth-resource"
))]
pub mod oauth {
    /// RFC 6749 §1.1's client: the Authorization Code flow with PKCE.
    #[cfg(feature = "oauth-client")]
    pub mod client {
        #[doc(no_inline)]
        pub use nest_rs_oauth_client::*;
    }
    /// RFC 6749 §1.1's resource server: RFC 9728's Protected Resource Metadata.
    #[cfg(feature = "oauth-resource")]
    pub mod resource {
        #[doc(no_inline)]
        pub use nest_rs_oauth_resource::*;
    }
    /// RFC 6749 §1.1's authorization server: client authentication and the token response.
    #[cfg(feature = "oauth-server")]
    pub mod server {
        #[doc(no_inline)]
        pub use nest_rs_oauth_server::*;
    }
}
/// Social login: the `SocialProvider` contract and its registry.
#[cfg(feature = "social")]
pub mod social {
    #[doc(no_inline)]
    pub use nest_rs_social::*;
}

/// OpenTelemetry: traces, metrics, logs and the OTLP export.
#[cfg(feature = "opentelemetry")]
pub mod opentelemetry {
    #[doc(no_inline)]
    pub use nest_rs_opentelemetry::*;
}

/// The OpenAPI 3.1 document and its Swagger UI.
#[cfg(feature = "openapi")]
pub mod openapi {
    #[doc(no_inline)]
    pub use nest_rs_openapi::*;
}

/// Liveness, readiness and startup probes, and their indicators.
#[cfg(feature = "health")]
pub mod health {
    #[doc(no_inline)]
    pub use nest_rs_health::*;
}

/// Rate limiting: `ThrottlerGuard` and the `ThrottlerStore` port.
#[cfg(feature = "throttler")]
pub mod throttler {
    #[doc(no_inline)]
    pub use nest_rs_throttler::*;
}

/// The W3C Server-Timing response header.
#[cfg(feature = "server-timing")]
pub mod server_timing {
    #[doc(no_inline)]
    pub use nest_rs_server_timing::*;
}

/// Static files and single-page apps, served beside the API.
#[cfg(feature = "static-files")]
pub mod static_files {
    #[doc(no_inline)]
    pub use nest_rs_static_files::*;
}

/// The in-process test harness: `TestApp`, its client and provider overrides.
#[cfg(feature = "testing")]
pub mod testing {
    #[doc(no_inline)]
    pub use nest_rs_testing::*;
}

// The layer contracts an edge's decorators name, so a custom `Guard` or `Pipe`
// needs no per-crate dependency; `Interceptor` and `ExceptionFilter` are core's.
/// The guard contract every edge checks, and the app-wide `GlobalGuards`.
#[cfg(feature = "guards")]
pub mod guards {
    #[doc(no_inline)]
    pub use nest_rs_guards::*;
}

/// Validation and transformation pipes.
#[cfg(feature = "pipes")]
pub mod pipes {
    #[doc(no_inline)]
    pub use nest_rs_pipes::*;
}

/// Object storage over `object_store`, its backend picked by the URL's scheme.
#[cfg(feature = "storage")]
pub mod storage {
    #[doc(no_inline)]
    pub use nest_rs_storage::*;
}

// What codegen appends after an expansion reaching a feature an attribute
// pulls in (`#[authorize]`, `#[crud]`): one chain per expansion, each link
// adding its feature when this build lacks it, the last saying one sentence
// for them all. Written out pair by pair: a `#[macro_export]` a macro produces
// is unreachable by an absolute path inside its own crate, and a `#[cfg]` a
// `macro_rules!` emits is read in the caller's crate.

#[cfg(feature = "authz")]
#[doc(hidden)]
#[macro_export]
macro_rules! __requires_authz {
    ($what:literal $req:literal [$($missing:literal)*] $next:ident $($rest:ident)*) => {
        $crate::$next! { $what $req [$($missing)*] $($rest)* }
    };
}

#[cfg(not(feature = "authz"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __requires_authz {
    ($what:literal $req:literal [$($missing:literal)*] $next:ident $($rest:ident)*) => {
        $crate::$next! { $what $req [$($missing)* "authz"] $($rest)* }
    };
}

#[cfg(feature = "seaorm")]
#[doc(hidden)]
#[macro_export]
macro_rules! __requires_seaorm {
    ($what:literal $req:literal [$($missing:literal)*] $next:ident $($rest:ident)*) => {
        $crate::$next! { $what $req [$($missing)*] $($rest)* }
    };
}

#[cfg(not(feature = "seaorm"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __requires_seaorm {
    ($what:literal $req:literal [$($missing:literal)*] $next:ident $($rest:ident)*) => {
        $crate::$next! { $what $req [$($missing)* "seaorm"] $($rest)* }
    };
}

#[cfg(feature = "graphql")]
#[doc(hidden)]
#[macro_export]
macro_rules! __requires_graphql {
    ($what:literal $req:literal [$($missing:literal)*] $next:ident $($rest:ident)*) => {
        $crate::$next! { $what $req [$($missing)*] $($rest)* }
    };
}

#[cfg(not(feature = "graphql"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __requires_graphql {
    ($what:literal $req:literal [$($missing:literal)*] $next:ident $($rest:ident)*) => {
        $crate::$next! { $what $req [$($missing)* "graphql"] $($rest)* }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __requires_end {
    ($what:literal $req:literal []) => {};
    ($what:literal $req:literal [$only:literal]) => {
        ::core::compile_error!(::core::concat!(
            $what, " expands into `nest_rs::", $only,
            "`, which this build does not enable. Add the feature to Cargo.toml:\n\n    \
             nest-rs = { version = \"", $req, "\", features = [\"", $only, "\"] }"
        ));
    };
    ($what:literal $req:literal [$first:literal $($more:literal)+]) => {
        ::core::compile_error!(::core::concat!(
            $what, " expands into `nest_rs::", $first, "`", $(" and `nest_rs::", $more, "`",)+
            ", which this build does not enable. Add the features to Cargo.toml:\n\n    \
             nest-rs = { version = \"", $req, "\", features = [\"", $first, "\""
            $(, ", \"", $more, "\"")+, "] }"
        ));
    };
}

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
        Ambient, App, AppBuilder, Container, ContainerBuilder, Module, Reflector, Scoped,
        async_trait, hooks, injectable, interceptor, module,
    };

    // Outside every edge's feature: a headless worker needs `#[input]` too.
    pub use nest_rs_core::{input, output, wire_enum};

    #[cfg(feature = "http")]
    pub use nest_rs_http::{
        ClientIp, HttpConfig, HttpModule, Json, Path, Query, RawBody, controller, http_code,
        redirect, response_header, routes,
    };

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
    pub use nest_rs_seaorm::expose;

    #[cfg(feature = "schedule")]
    pub use nest_rs_schedule::scheduled;

    #[cfg(feature = "ws")]
    pub use nest_rs_ws::{gateway, messages};

    // No `#[crud]`: the HTTP and GraphQL decorators share the name, so a glob
    // would collide; import it from its edge (`nest_rs::graphql::crud`).
}
