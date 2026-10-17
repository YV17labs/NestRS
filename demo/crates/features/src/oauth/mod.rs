pub const TARGET: &str = "features::oauth";

mod config;
mod dtos;
mod module;
mod scope;
mod service;
mod strategies;

pub mod http;

pub use config::{ClientPayload, OAuthConfig};
pub use dtos::LoginDto;
pub use module::OAuthModule;
pub use nest_rs::oauth::server::RegisteredClient;
pub use scope::role_from_db;
pub use service::{AuthenticatedClient, Caller, OAuthService};
pub use strategies::{ClientAuthnGuard, OAuthGuard};

pub use http::OAuthHttpModule;
