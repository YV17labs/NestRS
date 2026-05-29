//! In-process testing harness for nestrs.
//!
//! [`TestApp`] boots an app's real dependency-injection graph — the same
//! four-phase [`AppBuilder`](nestrs_core::AppBuilder) build production uses, with
//! the access-graph contract enforced — and exposes its HTTP surface through
//! `poem`'s `TestClient` without binding a socket. Because GraphQL, OpenAPI and
//! MCP all self-mount as HTTP endpoints, a single client exercises every surface,
//! so wiring that previously only surfaced under `curl` against a running binary
//! is now reachable from `cargo test`.
//!
//! Swap a real provider for a fake with [`TestAppBuilder::override_dyn`] /
//! [`override_value`](TestAppBuilder::override_value) — the NestJS
//! `overrideProvider` analog.
//!
//! ```ignore
//! use nestrs_testing::TestApp;
//!
//! let app = TestApp::for_module::<AppModule>().await?;
//! let resp = app.http().get("/users").send().await;
//! resp.assert_status_is_ok();
//!
//! // With a mock swapped in:
//! let app = TestApp::builder()
//!     .module::<AppModule>()
//!     .override_dyn::<dyn Clock>(std::sync::Arc::new(FrozenClock))
//!     .build()
//!     .await?;
//! ```

mod app;
mod headless;

#[cfg(feature = "orm")]
mod database;
#[cfg(feature = "orm")]
pub use database::EphemeralDatabase;

pub use app::{TestApp, TestAppBuilder};
pub use headless::{HeadlessApp, TransportHandle};

/// `poem`'s test client and its assertion helpers, re-exported so a test crate
/// needs no direct `poem` dependency — mirroring how each surface wraps its
/// backing crate.
pub use poem::test::{TestClient, TestForm, TestJson, TestRequestBuilder, TestResponse};
