//! `#[crud]` generates all five operations unless `ops` narrows them, each
//! typed off its service's trait: one the service does not offer — here a
//! create on a read-only service, no `Creatable` impl — is refused naming the
//! trait and the op, never a no-op mutation on the wire.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "read_only_crud_notes")]
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

#[crud(service = NotesService, ops = [get, create])]
impl NotesController {}

fn main() {}
