mod dto;
mod entities;
mod module;
mod service;

pub mod graphql;
pub mod http;
pub mod mcp;
pub mod ws;

pub use dto::PersonDto;
pub use entities::user::*;
pub use module::UsersModule;
pub use service::{SocialIdentity, UsersService};

pub use graphql::UsersGraphqlModule;
pub use http::UsersHttpModule;
pub use mcp::UsersMcpModule;
pub use ws::UsersWsModule;
