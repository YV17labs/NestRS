//! **Entity** template — one `#[expose]`d SeaORM entity (`g entity`).
//!
//! [`resource::ENTITY`](super::resource::ENTITY) without `service = …`: a
//! `CrudService` owns exactly one entity and `g entity` writes no service, so the
//! link is the developer's to declare. The file names no `super::` path, so it
//! compiles at `entity.rs` or in `entities/`; its columns match what
//! `nestrs g migration create_<name>` scaffolds.

pub(crate) const ENTITY: &str = r#"use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "{{entity}}", soft_delete, timestamps)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "{{table}}",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
    #[expose]
    pub updated_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
}
"#;

/// The index of a module that owns several entities, written only when the
/// folder exists without one.
pub(crate) const ENTITIES_MOD: &str = r#"pub mod {{stem}};
"#;
