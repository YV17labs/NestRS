//! `bind = Service` reads the subject's id from the route's one path
//! parameter, and a route's path opens with its controller's: a prefix naming
//! a parameter beside the route's `{id}` leaves the id ambiguous, so the route
//! is refused at compile time rather than answering `500` on every request.

use nest_rs::authz::Read;
use nest_rs::http::{controller, routes};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "prefixed_bind_notes")]
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

#[controller(path = "/orgs/{org}/notes")]
struct NotesController;

#[routes]
impl NotesController {
    #[get("/{id}")]
    #[authorize(Read, bind = NotesService)]
    async fn get(&self, note: Authorized<Read, note::Entity>) -> String {
        note.id.to_string()
    }
}

fn main() {}
