//! OAuth 2.0 **client** for nestrs — this app acting as the party that obtains
//! a token from somebody else's authorization server.
//!
//! RFC 6749 §1.1 names four roles, and this crate is exactly one of them. It
//! holds the Authorization Code flow with PKCE ([`OAuthClient`]), the signed
//! CSRF/PKCE transaction that lets the round-trip work without server-side
//! session storage, and the `for_root` seam that wires one configured client as
//! global infrastructure ([`OAuthClientModule`]).
//!
//! What it deliberately does **not** hold is the mirror direction:
//! authenticating somebody else's registered client at *our* token endpoint is
//! credential verification, so it lives in `nest-rs-authn` beside the password
//! and token paths. The two used to share a module named `oauth`, where a
//! reader could not tell which direction a symbol faced.
//!
//! For **social login** — mounting GitHub/Google or a custom provider behind an
//! open, discovered provider contract — reach for `nest-rs-social`, whose
//! providers compose this client as their shared flow.
//!
//! Integration tests: `tests/integration/main.rs`, with paths mirroring `src/`.

#![warn(missing_docs)]

/// This crate's span target — the outbound OAuth flow: redirect, callback
/// refusal, token exchange, userinfo.
///
/// Declared by the crate that **owns** the concern, so a target's one job — to
/// say *where* an event came from — stays true.
pub const TARGET: &str = "nest_rs::oauth::client";

mod client;
mod config;
mod module;

pub use client::{AuthorizationRedirect, OAuthClient, TokenSet};
pub use config::OAuthClientConfig;
pub use module::{OAuthClientModule, OAuthClientSetup};
