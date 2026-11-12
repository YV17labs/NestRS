pub const TARGET: &str = "features::sign_in";

mod config;
mod dtos;
mod module;
mod service;

pub mod http;

pub use config::{ClientPayload, SignInConfig};
pub use dtos::{AuthorizeDto, LoginDto};
pub use module::SignInModule;
pub use service::{AuthenticatedClient, Caller, SignInService};

pub use http::SignInHttpModule;
