//! OpenAPI 3.1 + Swagger UI for nestrs.
//!
//! Import [`OpenApiModule`] and the HTTP transport serves `GET /api-json` (the
//! document, composed from every `#[controller]` linked into the binary) and
//! `GET /api` (bundled, offline Swagger UI). Request/response schemas come from
//! the `Json<T>` payload types via [`schemars::JsonSchema`].

mod config;
mod document;
mod module;
mod ui;

pub use config::OpenApiConfig;
pub use module::{OpenApiModule, OpenApiSetup};
