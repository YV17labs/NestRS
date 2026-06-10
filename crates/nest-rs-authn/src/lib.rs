//! Authentication for nestrs — establishing *who* the caller is.
//!
//! Integration tests: `tests/authn.rs` + paths mirroring `src/` (see CLAUDE.md).
//! Gaps: `jwt/module.rs`, `oauth/module.rs` (app e2e); live OAuth HTTP (app e2e).
//!
//! Composable framework concerns (product wiring lives in `product`):
//! - [`jwt`] — token sign/verify + [`AuthnModule`]
//! - [`oauth`] — Authorization Code client + [`OAuth2Module`]
//! - [`passport`] — [`Strategy`], [`AuthGuard`], [`JwtStrategy`]
//! - [`password`] — Argon2 helpers (no DI module)
//!
//! **Naming convention.** A `*Service` is a singleton DI provider that holds
//! stateful infrastructure (key material, in-memory caches) — [`JwtService`]
//! is built once at boot and injected wherever a token is signed or verified.
//! A `*Client` is a transient builder over an external API surface —
//! [`OAuth2Client`] is constructed per flow (authorize → exchange → userinfo)
//! and carries no shared state between callers.

pub mod jwt;
pub mod oauth;
pub mod passport;
pub mod password;

mod error;

pub use error::{AuthError, CredentialError};
pub use jwt::{AuthnModule, AuthnSetup, JwtConfig, JwtKey, JwtOptions, JwtService};
pub use oauth::{Authorization, OAuth2Client, OAuth2Config, OAuth2Module, OAuth2Setup, TokenError};
pub use passport::{AuthGuard, JwtStrategy, Strategy, basic_credentials, bearer_token};
pub use password::{PasswordError, burn_verify, hash_password, verify_password};

/// Re-exported so apps configure [`JwtOptions`] without a direct `jsonwebtoken` dependency.
pub use jsonwebtoken::Algorithm;
