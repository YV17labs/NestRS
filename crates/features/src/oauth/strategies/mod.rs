//! Authorization Code (browser) and Client Credentials (machine) strategies
//! for the OAuth2 feature. Each is a generic `nest_rs_authn::Strategy` impl;
//! `<Feature>Module` registers them alongside their `AuthGuard` alias.

mod client_credentials;
mod oauth;

pub use client_credentials::{ClientAuthGuard, ClientCredentialsStrategy};
pub use oauth::{OAuthGuard, OAuthStrategy};
