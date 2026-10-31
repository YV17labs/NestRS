//! `service = <Type>` names the service every generated operation delegates
//! to, through the host's `#[inject]` field of that type: a service the host
//! does not inject is refused once, at the block, saying what to add.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "misnamed_crud_notes")]
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

#[injectable]
#[derive(Default)]
struct ArchiveService;

impl CrudService for ArchiveService {
    type Entity = note::Entity;
}

#[controller(path = "/notes")]
struct NotesController {
    #[inject]
    notes: Arc<NotesService>,
}

#[crud(service = ArchiveService, ops = [get])]
impl NotesController {}

fn main() {}
