//! Users core — the port (entity + service + DTOs + errors). Importing
//! [`UsersCoreModule`] from another feature gives access to [`UsersService`]
//! without exposing any transport surface.

mod entity;
mod error;
mod module;
mod service;

pub use entity::*;
pub use error::{CredentialError, UserError};
pub use module::UsersCoreModule;
pub use service::{UsersService, UsersServiceByName, UsersServiceByOrg};
