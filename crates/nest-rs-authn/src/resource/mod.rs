//! OAuth 2.0 protected-resource surface (RFC 9728) — the server half of the
//! discovery flow every MCP, HTTP and WS client needs before it can obtain a
//! token.

mod config;
mod controller;
mod interceptor;
mod metadata;
mod module;

pub use config::ProtectedResourceConfig;
pub use controller::ProtectedResourceController;
pub use interceptor::NoBearerChallenge;
pub use metadata::{ProtectedResourceMetadata, WELL_KNOWN_PATH};
pub use module::{ProtectedResourceModule, ProtectedResourceSetup};
