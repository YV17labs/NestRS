//! Authentication for nestrs — establishing *who* the caller is, the counterpart
//! to `nestrs-authz`'s *what they may do*.
//!
//! Three pieces, mirroring the NestJS `@nestjs/jwt` + Passport surface:
//!
//! - [`JwtService`] signs and verifies tokens ([`AuthnModule::for_root`] makes a
//!   configured one injectable everywhere).
//! - [`Strategy`] turns a request into an authenticated principal; it is a plain
//!   `#[injectable]` provider implementing one method.
//! - [`AuthGuard<S>`] runs a strategy per route and attaches the principal to the
//!   request, where a later guard (`AbilityGuard`) or a `Ctx<Principal>` extractor
//!   reads it.
//!
//! One [`Strategy`] trait serves both stateless bearer tokens and redirect-based
//! OAuth because [`Outcome`] lets a strategy either authenticate or challenge the
//! client (see [`oauth`]).

mod error;
mod guard;
mod jwt;
mod module;
mod oauth;
mod strategy;

pub use error::AuthError;
pub use guard::AuthGuard;
pub use jwt::{JwtConfig, JwtKey, JwtOptions, JwtService};
pub use module::{AuthnModule, AuthnSetup, OAuth2Module, OAuth2Setup};
pub use oauth::{Authorization, OAuth2Client, OAuth2Config};
pub use strategy::{basic_credentials, bearer_token, JwtStrategy, Outcome, Strategy};

/// Re-exported so apps configure [`JwtOptions`] without taking a direct
/// `jsonwebtoken` dependency.
pub use jsonwebtoken::Algorithm;
