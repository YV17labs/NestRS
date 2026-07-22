//! Posts — the tutorial feature. `publish` is the framework's atomicity
//! showcase: it writes the status update and a `post_publication` audit row in
//! one ambient request transaction, so a failure in either unwinds both.

mod entities;
mod error;
mod event;
mod module;
mod service;

pub mod graphql;
pub mod http;
pub mod mcp;

pub use entities::post::*;
/// The publish audit log entity, kept internal to the feature (no wire
/// surface). Re-exported so persistence tests can assert the audit row that
/// `PostsService::publish` writes alongside the status update.
pub use entities::publication;
pub use error::PostError;
pub use event::PostPublishedEvent;
pub use module::PostsModule;
pub use service::PostsService;

pub use graphql::PostsGraphqlModule;
pub use http::PostsHttpModule;
pub use mcp::PostsMcpModule;
