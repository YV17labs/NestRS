//! Authentication for nestrs — establishing *who* the caller is, and nothing
//! else.
//!
//! [`Strategy`] turns a request into a principal; [`AuthnGuard`] runs one and
//! records the resulting [`actor_id`](PrincipalIdentity::actor_id) as the audit
//! identity every downstream event inherits. [`JwtService`] signs and verifies;
//! [`hash_password`] / [`verify_password`] cover the local-credential case.
//! The product wiring that binds them — the concrete claims type, the user
//! lookup, the lockout policy — belongs to the consuming application (in this
//! repo, `demo/crates/features`).
//!
//! **What lives elsewhere.** The RFC 6749 §1.1 roles have their own crates, so
//! an app links the one it plays:
//!
//! | Concern | Crate |
//! |---|---|
//! | obtaining a token from someone else's authorization server | `nest-rs-oauth-client` |
//! | issuing tokens — §5.2 error codes, §2.3.1 client authentication | `nest-rs-oauth-server` |
//! | RFC 9728 discovery — what this deployment is, and where to get a token | `nest-rs-oauth-resource` |
//! | social login behind a discovered provider contract | `nest-rs-social` |

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — principal resolution: strategies, credential
/// verification, and the guard's authentication outcome.
pub const TARGET: &str = "nest_rs::authn";

mod config;
mod credentials;
mod error;
mod guard;
mod module;
mod password;
mod principal;
pub mod scope;
mod service;
mod strategies;
mod strategy;

pub use config::AuthnConfig;
pub use credentials::{basic_credentials, bearer_token};
pub use error::{AuthError, CredentialError, PasswordError};
pub use guard::AuthnGuard;
pub use module::{AuthnModule, AuthnSetup};
pub use password::{burn_verify, hash_password, verify_password};
pub use principal::PrincipalIdentity;
pub use service::{JwtKey, JwtOptions, JwtService};
pub use strategies::JwtStrategy;
pub use strategy::{AUTHENTICATE_TIMEOUT, Strategy};

/// Re-exported so apps configure [`JwtOptions`] without a direct `jsonwebtoken` dependency.
pub use jsonwebtoken::Algorithm;
