//! OAuth feature — token issuance (`core/`) + HTTP endpoints (`http/`).
//!
//! Token issuance: strategies turn a provider login (or client credentials)
//! into [`Claims`](crate::Claims); [`TokenIssuer`] signs them. The `auth` app
//! imports [`OAuthHttpModule`] to expose `/token`, `/authorize`, `/callback`,
//! and `/login`. Another app could import only [`OAuthCoreModule`] to sign
//! tokens internally without serving any HTTP route.

pub mod core;
pub mod http;

pub use core::*;
pub use http::{OAuthController, OAuthHttpModule};
