mod config;
mod dto;
mod module;
mod scope;
mod service;
mod strategies;

pub mod http;

pub use config::{IssuerConfig, RegisteredClient};
pub use dto::{LoginInput, TokenRequest};
pub use module::OAuthModule;
pub use scope::{role_from_db, roles_for_scope};
pub use service::{AccessToken, AuthenticatedClient, Caller, OAuthService};
pub use strategies::{ClientAuthGuard, ClientCredentialsStrategy, OAuthGuard, OAuthStrategy};

pub use http::{OAuthController, OAuthHttpModule};
