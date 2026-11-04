//! `bind = Service` loads a subject for the `Authorized<Action, E>` parameter that
//! receives it: with none, it is refused by name.

use nest_rs::mcp::{McpError, mcp, tools};
use nest_rs::seaorm::CrudService;

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "unreceived_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct Read;
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
    #[authorize(Read, note::Entity, bind = NotesService)]
    async fn note(&self, id: uuid_free::Id) -> Result<String, McpError> {
        Ok(id.0)
    }
}

mod uuid_free {
    pub struct Id(pub String);

    impl std::str::FromStr for Id {
        type Err = std::convert::Infallible;

        fn from_str(text: &str) -> Result<Self, Self::Err> {
            Ok(Self(text.to_owned()))
        }
    }
}

fn main() {}
