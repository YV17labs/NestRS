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

mod app;
mod env;
mod headless;
mod logs;
pub mod mcp;

#[cfg(feature = "orm")]
mod database;
#[cfg(feature = "orm")]
pub use database::EphemeralDatabase;

/// Driving a GraphQL subscription over graphql-ws (feature `graphql`).
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "graphql")]
pub use graphql::{GraphqlSocket, GraphqlSocketBuilder};

pub use app::{TestApp, TestAppBuilder};
pub use env::load_project_env;
pub use headless::{HeadlessApp, TransportHandle};
pub use logs::{CapturedEvent, LogCapture};

pub use poem::test::{TestClient, TestForm, TestJson, TestRequestBuilder, TestResponse};
