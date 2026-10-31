//! `#[crud]` reaches the injected fields `#[controller]` declares beside the
//! struct, private to its module, so a block written in a module neither it nor
//! below it deliberately fails to resolve: rustc's privacy error leads, and the
//! refusal naming the struct and where to write the block follows it.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::controller;
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "elsewhere_crud_notes")]
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
pub struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

mod host {
    use super::*;

    #[controller(path = "/notes")]
    pub struct NotesController {
        #[inject]
        pub svc: Arc<NotesService>,
    }
}

mod elsewhere {
    use nest_rs::http::crud;

    #[crud(service = svc, ops = [get])]
    impl super::host::NotesController {}
}

fn main() {}
