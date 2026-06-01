//! OpenAPI 3.1 + Swagger UI for nestrs — the REST analog of
//! [`nestrs_graphql::GraphqlModule`].
//!
//! Import [`OpenApiModule`] in a `#[module(imports = [...])]` and the HTTP
//! transport serves (at the NestJS-convention paths — the spec mandates none):
//! - `GET /api-json` — the OpenAPI document, composed from the controllers
//!   linked into the binary (no central route list).
//! - `GET /api` — a bundled, offline Swagger UI rendering that document.
//!
//! The document is built from the same `HttpControllerMeta`s the transport
//! mounts (read via [`nestrs_core::DiscoveryService`]), so any route added to a
//! `#[controller]` appears in the docs with no extra wiring. The request/response
//! schemas come from the `Json<T>` payload types the `#[routes]` macro records;
//! `T` must implement [`schemars::JsonSchema`] (typically via `#[derive]`), the
//! same trait MCP tool parameters use.
//!
//! Swagger UI is vendored (`assets/`, from `swagger-ui-dist` 5.32.6) and embedded
//! in the binary — no CDN, works offline.

mod config;
mod document;
mod module;
mod ui;

pub use config::OpenApiConfig;
pub use module::{OpenApiModule, OpenApiSetup};
