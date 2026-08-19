//! OpenAPI 3.1 + Swagger UI for nestrs.
//!
//! Import [`OpenApiModule`] and the HTTP transport serves `GET /api-json` (the
//! document, composed from every `#[controller]` linked into the binary) and
//! `GET /api` (bundled, offline Swagger UI). Request/response schemas come from
//! the `Json<T>` payload types via [`schemars::JsonSchema`].

#![warn(missing_docs)]

/// This crate's span target — Document composition and the mounted UI.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::openapi";

mod config;
mod document;
mod module;
mod ui;

pub use config::OpenApiConfig;
pub use module::{OpenApiModule, OpenApiSetup};
