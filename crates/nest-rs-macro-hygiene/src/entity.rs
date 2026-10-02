//! `#[expose]` on a real entity, in a crate whose one dependency is the
//! umbrella.
//!
//! sea-orm's derives root their expansion at a *relative* `sea_orm::`, so the
//! `use` below is what they resolve against — the entity names no `sea-orm`
//! line, and anything `#[expose]` emits beyond it has to resolve through the
//! umbrella too. Both arms are compiled: the HTTP-only one, and the GraphQL one
//! under `graphql`, which adds the object derive and the relation loader.

use nest_rs::core::serde::{Deserialize, Serialize};
use nest_rs::resource::expose;
use nest_rs::seaorm::sea_orm;
use sea_orm::entity::prelude::*;

#[cfg_attr(
    not(feature = "graphql"),
    expose(name = "HygieneNote", soft_delete, timestamps)
)]
#[cfg_attr(
    feature = "graphql",
    expose(name = "HygieneNote", graphql, soft_delete, timestamps)
)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "hygiene_note",
    model_attrs(
        derive(Serialize, Deserialize),
        serde(crate = "::nest_rs::core::serde")
    )
)]
pub struct Model {
    /// The key `#[crud]` routes on, typed through sea-orm's own prelude.
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    /// The one column both generated inputs carry.
    #[expose(input(create, update), validate(length(min = 1)))]
    pub body: String,
    /// Written by the `timestamps` lifecycle.
    #[expose]
    pub created_at: DateTimeWithTimeZone,
    /// Written by the `timestamps` lifecycle.
    #[expose]
    pub updated_at: DateTimeWithTimeZone,
    /// Written by the `soft_delete` lifecycle.
    pub deleted_at: Option<DateTimeWithTimeZone>,
}
