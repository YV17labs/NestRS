//! `bind = Service` loads and authorizes the service's own entity, while the
//! class gate and the response mask read the entity `#[authorize]` writes: a
//! written entity other than the service's would gate and mask one entity and
//! load another, so it does not compile.

use nest_rs::authz::Read;
use nest_rs::http::{controller, routes};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "mismatched_bind_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod gadget {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Gadget")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "mismatched_bind_gadgets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

#[controller(path = "/notes")]
struct NotesController;

#[routes]
impl NotesController {
    #[get("/{id}")]
    #[authorize(Read, gadget::Entity, bind = NotesService)]
    async fn get(&self, note: Authorized<Read, note::Entity>) -> String {
        note.id.to_string()
    }
}

fn main() {}
