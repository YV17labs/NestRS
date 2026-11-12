//! `bind = Service` loads the service's own entity, while the class gate and the
//! reads read the entity `#[authorize]` writes: a written entity other than the
//! service's would gate and read one entity and load another.

use nest_rs::authz::Read;
use nest_rs::mcp::{McpError, mcp, tools};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "mismatched_resource_notes")]
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
    #[sea_orm(table_name = "mismatched_resource_gadgets")]
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

#[mcp(path = "/notes")]
#[derive(Clone, Default)]
struct NotesTool;

#[tools]
impl NotesTool {
    /// A note.
    #[resource(template = "note://{id}")]
    #[authorize(Read, gadget::Entity, bind = NotesService)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> Result<String, McpError> {
        Ok(note.id.to_string())
    }
}

fn main() {}
