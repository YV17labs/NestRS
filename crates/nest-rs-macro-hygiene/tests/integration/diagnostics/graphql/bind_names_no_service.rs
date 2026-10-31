//! `bind =` on an operation names the service that loads the subject, as on a
//! route; an action marker there — the action and the service swapped — is
//! refused with the note saying so.

use nest_rs::authz::{Update, WireModelDefaults};
use nest_rs::graphql::{operations, resolver};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "swapped_bind_notes")]
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

#[resolver]
struct NotesResolver;

#[operations]
impl NotesResolver {
    #[mutation]
    #[authorize(Update, bind = Update)]
    async fn touch_note(&self, note: Authorized<Update, note::Entity>) -> bool {
        let _ = note;
        true
    }
}

fn main() {}
