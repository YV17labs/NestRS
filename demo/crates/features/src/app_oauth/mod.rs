mod config;
mod dtos;
mod module;
mod scope;
mod service;
mod strategies;

pub mod http;

pub use config::IssuerConfig;
pub use dtos::LoginDto;
pub use module::AppOAuthModule;
pub use nest_rs::oauth::server::RegisteredClient;
pub use scope::{role_from_db, roles_for_scope};
pub use service::{AppOAuthService, AuthenticatedClient, Caller};
pub use strategies::{AppOAuthGuard, ClientAuthnGuard};

pub use http::AppOAuthHttpModule;
