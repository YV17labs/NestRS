//! A tool answering a row, or taking one as its arguments, says what crosses a wire,
//! as a route does: a reply answers the exposed type and a write takes an input.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose(input(create, update))]
        pub title: String,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::mcp::{Json, McpError, Parameters, mcp, tools};

#[mcp]
pub struct PostsTools;

#[tools]
impl PostsTools {
    #[tool(description = "Answers a row")]
    #[public]
    fn answered(&self) -> Result<Json<post::Model>, McpError> {
        loop {}
    }

    #[tool(description = "Reads a row")]
    #[public]
    fn read(&self, Parameters(_row): Parameters<post::Model>) -> Result<String, McpError> {
        loop {}
    }
}

fn main() {}
