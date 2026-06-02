//! Orgs core — the port. Importing [`OrgsCoreModule`] gives access to
//! [`OrgsService`] without any transport surface.

mod entity;
mod error;
mod module;
mod service;

pub use entity::*;
pub use error::OrgError;
pub use module::OrgsCoreModule;
pub use service::*;
