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
//! **What lives elsewhere, and why.** RFC 6749 §1.1 names four roles and this
//! crate is none of them: it answers a question that precedes all four. The
//! roles have their own crates, so an app links the one it actually plays —
//! a worker that verifies a JWT off a queue message no longer compiles an HTTP
//! controller, a credential registry, or an outbound HTTP client:
//!
//! | Concern | Crate |
//! |---|---|
//! | obtaining a token from someone else's authorization server | `nest-rs-oauth-client` |
//! | issuing tokens — §5.2 error codes, §2.3.1 client authentication | `nest-rs-oauth-server` |
//! | RFC 9728 discovery — what this deployment is, and where to get a token | `nest-rs-oauth-resource` |
//! | social login behind a discovered provider contract | `nest-rs-social` |
//!
//! **Naming convention.** A `*Service` is a singleton DI provider holding
//! stateful infrastructure (key material, in-memory caches) — [`JwtService`] is
//! built once at boot and injected wherever a token is signed or verified.
//!
//! Integration tests: `tests/integration/main.rs`, with paths mirroring `src/`.

#![warn(missing_docs)]

/// This crate's span target — principal resolution: strategies, credential
/// verification, and the guard's authentication outcome.
///
/// Declared by the crate that **owns** the concern, because a target's one job
/// is to say **where** an event came from. A central table in the kernel would
/// have meant `nest-rs-core` holding a name for a concern it does not know
/// exists.
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

pub use config::JwtConfig;
pub use credentials::{basic_credentials, bearer_token};
pub use error::{AuthError, CredentialError, PasswordError};
pub use guard::AuthnGuard;
pub use module::{AuthnModule, AuthnSetup};
pub use password::{burn_verify, hash_password, verify_password};
pub use principal::PrincipalIdentity;
pub use service::{JwtKey, JwtOptions, JwtService};
pub use strategies::JwtStrategy;
pub use strategy::Strategy;

/// Re-exported so apps configure [`JwtOptions`] without a direct `jsonwebtoken` dependency.
pub use jsonwebtoken::Algorithm;
