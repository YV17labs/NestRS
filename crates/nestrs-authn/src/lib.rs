//! Authentication for nestrs — establishing *who* the caller is.
//!
//! Integration tests: `tests/authn.rs` + paths mirroring `src/` (see CLAUDE.md).
//! Gaps: `jwt/module.rs`, `oauth/module.rs` (app e2e); live OAuth HTTP (app e2e).
//!
//! Composable framework concerns (product wiring lives in `domain`):
//! - [`jwt`] — token sign/verify + [`AuthnModule`]
//! - [`oauth`] — Authorization Code client + [`OAuth2Module`]
//! - [`passport`] — [`Strategy`], [`AuthGuard`], [`JwtStrategy`]
//! - [`password`] — Argon2 helpers (no DI module)

pub mod jwt;
pub mod oauth;
pub mod passport;
pub mod password;

mod error;

pub use error::AuthError;
pub use jwt::{AuthnModule, AuthnSetup, JwtConfig, JwtKey, JwtOptions, JwtService};
pub use oauth::{Authorization, OAuth2Client, OAuth2Config, OAuth2Module, OAuth2Setup};
pub use passport::{
    basic_credentials, bearer_token, AuthGuard, JwtStrategy, Outcome, Strategy,
};
pub use password::{burn_verify, hash_password, verify_password, PasswordError};

/// Re-exported so apps configure [`JwtOptions`] without a direct `jsonwebtoken` dependency.
pub use jsonwebtoken::Algorithm;
