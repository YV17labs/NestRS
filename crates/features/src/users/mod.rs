//! Users feature — port (`core/`) + transport adapters (`http/`, `graphql/`,
//! `ws/`). An app composes by listing the edge modules it serves; the core
//! module is reachable transitively through any of them.
//!
//! `pub use core::*` flattens the entity surface (Model, Entity, Column,
//! ActiveModel, the wire `User`, the DTOs) at the feature root so a
//! cross-feature consumer writes `users::Column::OrgId` without traversing
//! `core::`.

pub mod core;
pub mod graphql;
pub mod http;
pub mod ws;

pub use core::*;
pub use graphql::{UsersGraphqlModule, UsersResolver};
pub use http::{UsersController, UsersHttpModule};
pub use ws::{UsersGateway, UsersWsModule};
