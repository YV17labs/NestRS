//! In-process testing harness for nestrs.
//!
//! [`TestApp`] boots an app's real DI graph (same four-phase
//! [`AppBuilder`](nest_rs_core::AppBuilder) build, access-graph enforced) and
//! exposes HTTP through `poem`'s `TestClient` without binding a socket. GraphQL,
//! OpenAPI and MCP self-mount over HTTP, so one client drives every surface.
//!
//! Override providers with [`override_dyn`](TestAppBuilder::override_dyn) /
//! [`override_value`](TestAppBuilder::override_value).
//!
//! [`LogCapture`] covers the other half of a transport's contract: the events
//! it emits. A denial that fails closed but logs nothing passes every response
//! assertion — and is exactly what nobody can debug at 3am.
#![cfg_attr(not(test), deny(unsafe_code))]
#![warn(missing_docs)]

// An edge module keeps its namespace and gets no root re-export: `graphql::`
// and `ws::` say which protocol a name belongs to, and that is the scheme every
// multi-edge crate here already follows (`nest-rs-authz`, `nest-rs-seaorm`).
// Re-exporting both ways gave each type two paths, and this crate was already
// spelling one of them two ways 200 lines apart.
mod app;
mod env;
mod headless;
mod logs;
pub mod mcp;

#[cfg(feature = "orm")]
mod database;
#[cfg(feature = "orm")]
pub use database::EphemeralDatabase;

/// Driving a GraphQL subscription over graphql-transport-ws (feature
/// `graphql`).
#[cfg(feature = "graphql")]
pub mod graphql;

/// Driving a WS gateway over a real upgrade (feature `ws`).
#[cfg(feature = "ws")]
pub mod ws;

pub use app::{TestApp, TestAppBuilder};
pub use env::load_project_env;
pub use headless::{HeadlessApp, TransportHandle};
pub use logs::{CapturedEvent, CapturedSpan, LogCapture};

pub use poem::test::{TestClient, TestForm, TestJson, TestRequestBuilder, TestResponse};
