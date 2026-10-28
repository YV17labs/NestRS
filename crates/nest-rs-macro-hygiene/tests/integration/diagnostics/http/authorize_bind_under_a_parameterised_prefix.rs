//! `bind = Service` reads the subject's id from the route's one path
//! parameter, and a route's path opens with its controller's: a prefix naming
//! a parameter beside the route's `{id}` leaves the id ambiguous, so the route
//! is refused at compile time rather than answering `500` on every request.

use nest_rs::authz::{Read, WireModelDefaults};
use nest_rs::http::{controller, routes};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "prefixed_bind_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for note::Entity {}

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
