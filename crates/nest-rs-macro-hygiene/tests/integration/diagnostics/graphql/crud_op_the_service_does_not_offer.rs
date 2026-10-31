//! `#[crud]` on a resolver generates its operations off its service's traits
//! as on a controller: one the service does not offer — here a create on a
//! read-only service, no `Creatable` impl — is refused naming the trait and
//! the op, never a mutation missing from the schema.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::graphql::{crud, resolver};
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "read_only_resolver_notes")]
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

#[resolver]
struct NotesResolver {
    #[inject]
    svc: Arc<NotesService>,
}

#[crud(service = NotesService, ops = [get, create])]
impl NotesResolver {}

fn main() {}
