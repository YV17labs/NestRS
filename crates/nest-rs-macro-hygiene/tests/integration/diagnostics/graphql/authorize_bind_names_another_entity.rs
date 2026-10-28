//! `bind = Service` loads and authorizes the service's own entity, while the
//! class gate and the mask read the entity `#[authorize]` writes: a written
//! entity other than the service's would gate and mask one entity and load
//! another, so it does not compile — on GraphQL as on HTTP.

use nest_rs::authz::{Update, WireModelDefaults};
use nest_rs::graphql::{operations, resolver};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "mismatched_bind_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod gadget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "mismatched_bind_gadgets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for note::Entity {}
impl WireModelDefaults for gadget::Entity {}

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
