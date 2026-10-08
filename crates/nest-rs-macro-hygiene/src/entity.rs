//! `#[expose]` on a real entity, in a crate whose one dependency is the
//! umbrella.
//!
//! sea-orm's derives emit a *relative* `sea_orm::`, which the `use` below
//! satisfies — no `sea-orm` line.

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
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub body: String,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
    #[expose]
    pub updated_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
}
