//! Authorization Code (browser) and Client Credentials (machine) strategies
//! for the OAuth2 feature. Each is a generic `nestrs_authn::Strategy` impl;
//! `<Feature>CoreModule` registers them alongside their `AuthGuard` alias.

mod client_credentials;
mod oauth;

pub use client_credentials::{ClientAuthGuard, ClientCredentialsStrategy};
pub use oauth::{OAuthGuard, OAuthStrategy};
