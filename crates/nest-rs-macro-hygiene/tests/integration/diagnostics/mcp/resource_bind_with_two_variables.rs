//! `bind = Service` loads the subject by the one id the address names: a template
//! naming two variables has no one id to read, as a route of two path parameters
//! has none.

use nest_rs::authz::Read;
use nest_rs::mcp::{McpError, mcp, tools};
use nest_rs::seaorm::{Authorized, CrudService};

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "two_variable_notes")]
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
    #[resource(template = "note://{org}/{id}")]
    #[authorize(Read, bind = NotesService)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> Result<String, McpError> {
        Ok(note.id.to_string())
    }
}

fn main() {}
