//! A tool or a resource answering a row no `#[expose]` decorates says what crosses
//! a wire, as a route does, whatever holds the row and whatever the row derives.

mod secret {
    use sea_orm::entity::prelude::*;

    #[derive(
        Clone,
        Debug,
        PartialEq,
        DeriveEntityModel,
        serde::Serialize,
        nest_rs::mcp::rmcp::schemars::JsonSchema,
    )]
    #[schemars(crate = "nest_rs::mcp::rmcp::schemars")]
    #[sea_orm(table_name = "secrets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub password_hash: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::mcp::{Json, McpError, mcp, tools};

#[mcp(path = "/secrets")]
#[derive(Clone, Default)]
pub struct SecretsTools;

#[tools]
impl SecretsTools {
    #[tool(description = "Answers a row")]
    #[public]
    fn one(&self) -> Result<Json<secret::Model>, McpError> {
        loop {}
    }

    /// A row.
    #[resource(template = "secret://{id}")]
    #[public]
    async fn read(&self, id: String) -> Result<Json<Vec<secret::Model>>, McpError> {
        let _ = id;
        loop {}
    }
}

fn main() {}
