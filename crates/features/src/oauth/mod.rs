mod config;
mod dtos;
mod module;
mod scope;
mod service;
mod strategies;

pub mod http;

pub use config::IssuerConfig;
// Re-exported so apps wiring the issuer config name the client type via the feature.
pub use dtos::{AccessTokenDto, LoginDto, TokenRequestDto};
pub use module::OAuthModule;
pub use nest_rs_authn::RegisteredClient;
pub use scope::{role_from_db, roles_for_scope};
pub use service::{AuthenticatedClient, Caller, OAuthService};
pub use strategies::{ClientAuthGuard, ClientCredentialsStrategy, OAuthGuard, OAuthStrategy};

pub use http::{OAuthController, OAuthHttpModule};
