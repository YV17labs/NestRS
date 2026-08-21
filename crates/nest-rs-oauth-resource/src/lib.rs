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
//!   oauth-resource refusal opts out with `nest_rs_guards::NoBearerChallenge`,
//!   which lives below both this crate and the crates that write it.
//!
//! **Why this is not in `nest-rs-authn`.** That crate answers *who the caller
//! is*; this document answers *what this deployment is and where to get a
//! token*, which is a property of the deployment and true of callers who have
//! no identity yet. Keeping the two together meant a headless worker that only
//! verifies a JWT compiled an HTTP controller it never mounts and an
//! interceptor it never reaches.
//!
//! **Why `discovery` and not `resource-server`.** RFC 6749 §1.1's resource
//! server does two things — it *verifies tokens* and it is *discoverable* —
//! and only the second is here; verification is `nest-rs-authn`'s, whatever the
//! credential. A crate named for the whole role would invite the next
//! sender-constraint check (RFC 9449 DPoP, RFC 8705 mTLS) to land here, and
//! every one of those belongs beside the verifier. Named for what it holds,
//! the crate cannot mis-route a future addition: everything discovery grows —
//! more §2 members, `signed_metadata` — is already in scope, and everything
//! else is visibly out of it.
//!
//! Integration tests: `tests/integration/main.rs`, with paths mirroring `src/`.

#![warn(missing_docs)]

/// This crate's span target — RFC 9728 discovery: the metadata document, the
/// challenge stamped onto a `401`, and the boot-time audience binding.
pub const TARGET: &str = "nest_rs::oauth::resource";

mod config;
mod controller;
mod interceptor;
mod metadata;
mod module;

pub use config::OAuthResourceConfig;
pub use metadata::{ProtectedResourceMetadata, WELL_KNOWN_PATH};
pub use module::{OAuthResourceModule, OAuthResourceSetup};
