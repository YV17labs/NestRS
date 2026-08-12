//! **Entity** template — one `#[expose]`d SeaORM entity (`g entity`).
//!
//! [`resource::ENTITY`](super::resource::ENTITY) minus one argument, and the
//! omission is the whole design. `#[expose(service = …)]` names the
//! `CrudService` that **owns** this entity — the receiver of the generated
//! dataloaders, and the other half of the soft-delete boot audit. A service owns
//! exactly one entity (`CrudService::Entity` is an associated type), and
//! `g entity` writes no service, so there is never one it could name truthfully:
//! a plain `g feature` port's service is not a `CrudService` at all (naming it
//! fails to compile inside the macro expansion), and a resource port's service
//! already belongs to the entity it was generated with. The link is the
//! developer's to declare, and the printed next steps say so.
//!
//! Two consequences worth keeping: the file names **no `super::` path**, so it
//! compiles unchanged at `<feature>/entity.rs` or at
//! `<feature>/entities/<stem>.rs`; and `soft_delete` submits no
//! `SoftDeleteRegistration`, which is what makes a service-less exposure legal
//! rather than a boot failure.
//!
//! The columns match what `nestrs g migration create_<name>` scaffolds
//! (`created_at` / `updated_at` / `deleted_at`) — the two generators are written
//! in lockstep, so a resource is never left with a tombstone column nothing
//! honours.

pub const ENTITY: &str = r#"use nest_rs::resource::expose;
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

/// The index of a module that owns several entities. Written only when the
/// folder exists without one — the usual case is an `ensure_lines` edit onto the
/// index already there.
pub const ENTITIES_MOD: &str = r#"pub mod {{stem}};
"#;
