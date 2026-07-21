//! In-process testing harness for nestrs.
//!
//! [`TestApp`] boots an app's real DI graph (same four-phase
//! [`AppBuilder`](nest_rs_core::AppBuilder) build, access-graph enforced) and
//! exposes HTTP through `poem`'s `TestClient` without binding a socket. GraphQL,
//! OpenAPI and MCP self-mount over HTTP, so one client drives every surface.
//!
//! Override providers with [`override_dyn`](TestAppBuilder::override_dyn) /
//! [`override_value`](TestAppBuilder::override_value).
#![cfg_attr(not(test), deny(unsafe_code))]
#![warn(missing_docs)]

mod app;
mod env;
mod headless;
pub mod mcp;

#[cfg(feature = "orm")]
mod database;
#[cfg(feature = "orm")]
pub use database::EphemeralDatabase;

pub use app::{TestApp, TestAppBuilder};
pub use env::load_project_env;
pub use headless::{HeadlessApp, TransportHandle};

pub use poem::test::{TestClient, TestForm, TestJson, TestRequestBuilder, TestResponse};
