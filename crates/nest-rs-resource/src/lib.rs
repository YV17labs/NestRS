//! Expose a SeaORM entity to REST/OpenAPI from one declaration via [`macro@expose`].
//!
//! The wire DTO (`Serialize` + `JsonSchema`), CRUD input types, and
//! [`WireModelDefaults`] for response masking are always emitted.
//! Add the `graphql` flag on `#[expose(...)]` **and** enable the `graphql`
//! feature on this crate to also emit GraphQL types, auto-resolved relations,
//! and dataloaders.
//!
//! **Exposure is opt-in:** a column reaches the wire only when its field
//! carries `#[expose]` (or `#[expose(input(...))]`, which implies read). A field
//! with no `#[expose]` stays hidden on every transport, so a column added by a
//! later migration never leaks by omission.
//!
//! A column whose type is a **custom enum** passes through to the wire verbatim,
//! so the enum itself is what carries the wire traits: [`macro@wire_enum`] is
//! the enum mode of `#[expose]`, emitting them with the same `crate = `
//! overrides so the entity crate declares neither `schemars` nor
//! `async-graphql`.
//!
//! ```
//! # use nest_rs_resource::expose;
//! # mod service {
//! #     pub struct ItemsService;
//! # }
//! # mod rest {
//! #     use super::*;
//! #     use sea_orm::entity::prelude::*;
//! // HTTP / OpenAPI / masking — no GraphQL deps in the entity crate.
//! #[expose(name = "Item", service = super::service::ItemsService)]
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
//! #     #[sea_orm(table_name = "items")]
//! #     pub struct Model {
//! #         #[sea_orm(primary_key)]
//! #         #[expose]
//! #         pub id: i32,
//! #     }
//! #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
//! #     pub enum Relation {}
//! #     impl ActiveModelBehavior for ActiveModel {}
//! # }
//! # mod graph {
//! #     use super::*;
//! #     use sea_orm::entity::prelude::*;
//!
//! // + GraphQL surface (requires `features = ["graphql"]` on `nest-rs-resource`).
//! #[expose(name = "Item", service = super::service::ItemsService, graphql)]
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
//! #     #[sea_orm(table_name = "items")]
//! #     pub struct Model {
//! #         #[sea_orm(primary_key)]
//! #         #[expose]
//! #         pub id: i32,
//! #     }
//! #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
//! #     pub enum Relation {}
//! #     impl ActiveModelBehavior for ActiveModel {}
//! # }
//! # fn main() {
//! # use nest_rs_resource::graphql::async_graphql::OutputType;
//!
//! fn wire_only<T: serde::Serialize + schemars::JsonSchema>() {}
//! fn with_graphql<T: serde::Serialize + schemars::JsonSchema + OutputType>() {}
//! wire_only::<rest::Item>();
//! with_graphql::<graph::Item>();
//! # }
//! ```

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Relation dataloaders and their batches.
pub const TARGET: &str = "nest_rs::loader";

mod exposures;

/// Re-exports of the `async-graphql` primitives `#[expose(..., graphql)]`
/// emits, so generated code names them through this crate.
#[cfg(feature = "graphql")]
pub mod graphql {
    pub use nest_rs_graphql::async_graphql;
    pub use nest_rs_graphql::dataloader;
}

#[cfg(feature = "graphql")]
pub use exposures::relations::{PkLoadable, RelatedTo, RelationKey, RelationPage, SoleForeignKey};
pub use exposures::wire::WireModelDefaults;
/// Expose a SeaORM entity to REST/OpenAPI (and optionally GraphQL) from one
/// declaration.
///
/// ```
/// # use nest_rs_resource::{WireModelDefaults, expose};
/// # mod service {
/// #     pub struct UsersService;
/// # }
/// # mod user {
/// #     use super::*;
/// #     use sea_orm::entity::prelude::*;
/// #[expose(name = "User", service = super::service::UsersService)]
/// #[sea_orm::model]
/// # #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// # #[sea_orm(table_name = "users")]
/// pub struct Model {
///     #[sea_orm(primary_key, auto_increment = false)]
///     #[expose]                                                  // read-only
///     pub id: Uuid,
///     #[expose]                                                  // read-only
///     pub org_id: Uuid,
///     #[expose(input(create, update), validate(length(min = 1)))] // read + write
///     pub name: String,
///     #[expose(input(create), validate(email))]                  // read + create-only
///     pub email: String,
///     pub password_hash: Option<String>,                         // no #[expose] ⇒ hidden
/// }
/// # impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # fn main() -> Result<(), serde_json::Error> {
/// # use sea_orm::prelude::Uuid;
/// # use user::{CreateUser, Entity, Model, UpdateUser, User};
/// # let id = Uuid::nil();
///
/// let model = Model {
///     id,
///     org_id: id,
///     name: "Ada".into(),
///     email: "ada@example.com".into(),
///     password_hash: Some("$argon2id$…".into()),
/// };
/// assert_eq!(
///     serde_json::to_value(User::from(&model))?,
///     serde_json::json!({ "id": id, "org_id": id, "name": "Ada", "email": "ada@example.com" }),
/// );
/// let _ = CreateUser { name: "Ada".into(), email: "ada@example.com".into() };
/// let _ = UpdateUser { name: "Ada L.".into() };
///
/// fn implements<T: WireModelDefaults>() {}
/// implements::<Entity>();
/// # Ok(())
/// # }
/// ```
pub use nest_rs_resource_macros::expose;

/// The enum mode of [`macro@expose`]: make a column's enum type a wire type.
///
/// ```
/// # use nest_rs_resource::wire_enum;
/// # use sea_orm::entity::prelude::*;
/// #[wire_enum(graphql)]
/// #[derive(EnumIter, DeriveActiveEnum)]
/// #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
/// #[serde(rename_all = "lowercase")]
/// pub enum PostStatus {
///     #[sea_orm(string_value = "draft")]
///     Draft,
///     #[sea_orm(string_value = "published")]
///     Published,
/// }
/// # fn main() -> Result<(), serde_json::Error> {
/// # use nest_rs_resource::graphql::async_graphql::{InputType, OutputType};
///
/// fn implements<T: Copy + Eq + std::fmt::Debug + schemars::JsonSchema + InputType + OutputType>() {}
/// implements::<PostStatus>();
/// assert_eq!(serde_json::to_value(PostStatus::Published)?, "published");
/// assert_eq!(serde_json::from_value::<PostStatus>("draft".into())?, PostStatus::Draft);
/// # Ok(())
/// # }
/// ```
pub use nest_rs_resource_macros::wire_enum;

// Paths `#[expose]` emits, so an entity crate declares none of these; each
// derive carries a `crate = ` override pointing back here.
pub use async_trait::async_trait;
pub use serde_json;

#[doc(hidden)]
pub use chrono;
#[doc(hidden)]
pub use sea_orm;
#[doc(hidden)]
pub use uuid;

#[doc(hidden)]
pub use schemars;
#[doc(hidden)]
pub use serde;
pub use tracing;
#[doc(hidden)]
pub use validator;
