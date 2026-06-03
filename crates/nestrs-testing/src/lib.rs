//! In-process testing harness for nestrs.
//!
//! [`TestApp`] boots an app's real DI graph (same four-phase
//! [`AppBuilder`](nestrs_core::AppBuilder) build, access-graph enforced) and
//! exposes HTTP through `poem`'s `TestClient` without binding a socket. GraphQL,
//! OpenAPI and MCP self-mount over HTTP, so one client drives every surface.
//!
//! Override providers with [`override_dyn`](TestAppBuilder::override_dyn) /
//! [`override_value`](TestAppBuilder::override_value).

mod app;
mod headless;

#[cfg(feature = "orm")]
mod database;
#[cfg(feature = "orm")]
pub use database::EphemeralDatabase;

pub use app::{TestApp, TestAppBuilder};
pub use headless::{HeadlessApp, TransportHandle};

pub use poem::test::{TestClient, TestForm, TestJson, TestRequestBuilder, TestResponse};
