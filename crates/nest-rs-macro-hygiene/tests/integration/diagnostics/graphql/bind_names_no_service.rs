//! `bind =` on an operation names the service that loads the subject, as on a
//! route; an action marker there — the action and the service swapped — is
//! refused with the note saying so.

use nest_rs::authz::Update;
use nest_rs::graphql::{operations, resolver};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "swapped_bind_notes")]
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
