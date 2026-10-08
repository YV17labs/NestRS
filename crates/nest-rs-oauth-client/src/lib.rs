//! OAuth 2.0 **client** for nestrs — this app acting as the party that obtains
//! a token from somebody else's authorization server.
//!
//! It holds the Authorization Code flow with PKCE ([`OAuthClient`]) and the
//! `for_root` seam that wires one configured client ([`OAuthClientModule`]).
//! Authenticating a client at *our* token endpoint lives in `nest-rs-authn`; social
//! login (GitHub, Google, custom providers) in `nest-rs-social`, built on this client.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the outbound OAuth flow: redirect, callback
/// refusal, token exchange, userinfo.
pub const TARGET: &str = "nest_rs::oauth::client";

mod client;
mod config;
mod module;

pub use client::{AuthorizationRedirect, OAuthClient, TokenSet};
pub use config::OAuthClientConfig;
pub use module::{OAuthClientModule, OAuthClientSetup};
