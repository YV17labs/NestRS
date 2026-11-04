pub const TARGET: &str = "features::posts";

mod entities;
mod error;
mod event;
mod guard;
mod module;
mod service;
mod store;

pub mod graphql;
pub mod http;
pub mod mcp;

pub use entities::post::*;
pub use entities::publication;
pub use error::PostError;
pub use event::PostPublishedEvent;
pub use guard::PostAuthorGuard;
pub use module::PostsModule;
pub use service::PostsService;

pub use graphql::PostsGraphqlModule;
pub use http::PostsHttpModule;
pub use mcp::PostsMcpModule;
