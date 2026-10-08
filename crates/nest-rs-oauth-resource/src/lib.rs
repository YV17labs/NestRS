//! OAuth 2.0 **protected resource** for nestrs — the server half of the
//! discovery flow every MCP, HTTP and WS client walks before it can obtain a
//! token (RFC 9728).
//!
//! Two halves, both mounted by [`OAuthResourceModule`]:
//!
//! - the metadata document at [`WELL_KNOWN_PATH`], served to callers carrying
//!   no credential at all, and
//! - the interceptor that stamps the `resource_metadata` pointer onto every
//!   `401` — the one seam HTTP, WS and MCP share. A `401` that is not a
//!   oauth-resource refusal opts out with `nest_rs_guards::NoBearerChallenge`.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — RFC 9728 discovery: the metadata document, the
/// challenge stamped onto a `401`, and the boot-time audience binding.
pub const TARGET: &str = "nest_rs::oauth::resource";

mod audience;
mod config;
mod controller;
mod interceptor;
mod metadata;
mod module;

pub use config::OAuthResourceConfig;
pub use metadata::{ProtectedResourceMetadata, WELL_KNOWN_PATH};
pub use module::{OAuthResourceModule, OAuthResourceSetup};
