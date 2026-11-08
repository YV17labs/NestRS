//! `bind = Service` loads and authorizes the service's own entity, while the
//! class gate and the reads read the entity `#[authorize]` writes: a written
//! entity other than the service's would gate and read one entity and load
//! another, so it does not compile — on GraphQL as on HTTP.

use nest_rs::authz::Update;
use nest_rs::graphql::{operations, resolver};
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

#[resolver]
struct NotesResolver;

#[operations]
impl NotesResolver {
    #[mutation]
    #[authorize(Update, gadget::Entity, bind = NotesService)]
    async fn touch_note(&self, note: Authorized<Update, note::Entity>) -> bool {
        let _ = note;
        true
    }
}

fn main() {}
