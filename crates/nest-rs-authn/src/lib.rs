//! Authentication for nestrs — establishing *who* the caller is.
//!
//! Integration tests: `tests/integration/main.rs`, with paths mirroring `src/`.
//!
//! Composable framework concerns (product wiring lives in `product`):
//! - [`jwt`] — token sign/verify + [`AuthnModule`]
//! - [`oauth`] — Authorization Code client + [`OAuth2Module`]
//! - [`passport`] — [`Strategy`], [`AuthnGuard`], [`JwtStrategy`]
//! - [`password`] — Argon2 helpers (no DI module)
//! - [`resource`] — RFC 9728 discovery + [`ProtectedResourceModule`]
//! - [`scope`] — the space-delimited OAuth `scope` claim, as a `serde` helper
//!
//! **Naming convention.** A `*Service` is a singleton DI provider that holds
//! stateful infrastructure (key material, in-memory caches) — [`JwtService`]
//! is built once at boot and injected wherever a token is signed or verified.
//! A `*Client` is a transient builder over an external API surface —
//! [`OAuth2Client`] is constructed per flow (authorize → exchange → userinfo)
//! and carries no shared state between callers.
#![warn(missing_docs)]
pub mod jwt;
pub mod oauth;
pub mod passport;
pub mod password;
pub mod resource;
pub mod scope;

mod error;

pub use error::{AuthError, CredentialError};
pub use jwt::{AuthnModule, AuthnSetup, JwtConfig, JwtKey, JwtOptions, JwtService};
pub use oauth::{
    AuthenticatedClient, Authorization, OAuth2Client, OAuth2Config, OAuth2Module, OAuth2Setup,
    RegisteredClient, TokenError, TokenSet, authenticate_against_registry,
};
pub use passport::{
    AuthnGuard, JwtStrategy, PrincipalIdentity, Strategy, basic_credentials, bearer_token,
};
pub use password::{PasswordError, burn_verify, hash_password, verify_password};
pub use resource::{
    NoBearerChallenge, ProtectedResourceConfig, ProtectedResourceMetadata, ProtectedResourceModule,
    ProtectedResourceSetup, WELL_KNOWN_PATH,
};

/// Re-exported so apps configure [`JwtOptions`] without a direct `jsonwebtoken` dependency.
pub use jsonwebtoken::Algorithm;
