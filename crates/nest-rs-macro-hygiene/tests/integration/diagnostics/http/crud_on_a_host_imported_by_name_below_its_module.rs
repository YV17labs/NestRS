//! A block below the struct's module naming it through a named import passes
//! `#[crud]`'s check, which sees no import, and deliberately fails to resolve:
//! rustc's error on the trait is all it meets, where the path the refusal
//! prescribes, `impl super::NotesController`, reaches it.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::controller;
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "imported_crud_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

#[controller(path = "/notes")]
struct NotesController {
    #[inject]
    svc: Arc<NotesService>,
}

mod read {
    use nest_rs::http::crud;

    use super::NotesController;

    #[crud(service = svc, ops = [get])]
    impl NotesController {}
}

fn main() {}
