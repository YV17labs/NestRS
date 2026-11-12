//! OpenAPI 3.1 + Swagger UI for nestrs.
//!
//! Import [`OpenApiModule`] and the HTTP transport serves `GET /api-json` (the
//! document, composed from every `#[controller]` linked into the binary) and
//! `GET /api` (bundled, offline Swagger UI). Request/response schemas come from
//! the `Json<T>` payload types via [`schemars::JsonSchema`].

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — document composition and the mounted UI.
pub const TARGET: &str = "nest_rs::openapi";

mod config;
mod contact;
mod document;
mod license;
mod module;
mod ui;

pub use config::OpenApiConfig;
pub use contact::OpenApiContact;
pub use license::OpenApiLicense;
pub use module::{OpenApiModule, OpenApiSetup};
