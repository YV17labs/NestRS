//! `#[crud]`'s by-id operations read the id from the route's one path
//! parameter, and a route's path opens with its controller's: under a prefix
//! naming a parameter of its own, each would answer `500` on every request, so
//! the block is refused at compile time.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "prefixed_crud_notes")]
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

#[controller(path = "/orgs/{org}/notes")]
struct NotesController {
    #[inject]
    svc: Arc<NotesService>,
}

#[crud(service = svc, ops = [get])]
impl NotesController {}

fn main() {}
