//! `#[crud]` on a resolver reaches the injected fields `#[resolver]` declares
//! beside the struct by the struct's own name, as on a controller, so a block
//! naming its host through a type alias deliberately fails to resolve: rustc's
//! error on the trait leads, and the refusal naming the struct follows it.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::graphql::{crud, resolver};
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::{expose, sea_orm};
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "aliased_resolver_notes")]
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

type Notes = NotesResolver;

#[crud(service = svc, ops = [get])]
impl Notes {}

fn main() {}
