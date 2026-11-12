//! OAuth 2.0 **client** for nestrs — this app acting as the party that obtains
//! a token from somebody else's authorization server.
//!
//! It holds the Authorization Code flow with PKCE ([`OAuthClient`]), the answer
//! a handler returns for its authorization request ([`AuthorizationRedirect`])
//! and the module that wires one configured client ([`OAuthClientModule`]).
//! Authenticating a client at *our* token endpoint lives in
//! `nest-rs-oauth-server`; social login (GitHub, Google, custom providers) in
//! `nest-rs-social`, built on this client.
//!
//! **The authorization server is not proved at boot** (`nest_rs_core::prove`):
//! it is a third party's, reached once per login, and one briefly down must cost
//! those logins, never the app's start.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the outbound OAuth flow: redirect, callback
/// refusal, token exchange, userinfo.
pub const TARGET: &str = "nest_rs::oauth::client";

mod authorization_parameters;
mod client;
mod config;
mod module;
mod redirect;

pub use authorization_parameters::AuthorizationParameters;
pub use client::{AuthorizationResponse, OAuthClient, TokenSet};
pub use config::{OAuthClientConfig, refuse_plaintext_redirect};
pub use module::{OAuthClientModule, OAuthClientSetup};
pub use redirect::AuthorizationRedirect;
