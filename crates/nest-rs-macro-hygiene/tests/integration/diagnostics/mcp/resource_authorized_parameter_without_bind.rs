//! An `Authorized<Action, E>` parameter carries the row a `bind = Service` loads,
//! never the posture: written beside an `#[authorize]` that binds nothing, it is
//! refused by name rather than as a template variable it does not name.

use nest_rs::authz::Read;
use nest_rs::mcp::{McpError, mcp, tools};
use nest_rs::seaorm::Authorized;

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "unbound_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[mcp(path = "/notes")]
#[derive(Clone, Default)]
struct NotesTool;

#[tools]
impl NotesTool {
    /// A note.
    #[resource(template = "note://{id}")]
    #[authorize(Read, note::Entity)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> Result<String, McpError> {
        Ok(note.id.to_string())
    }
}

fn main() {}
