//! A prompt's messages are prose, opaque to the entity round-trip, so
//! `#[public(Action, Subject)]` cannot mask one, in the words `#[authorize]` meets
//! there, and names the `unmasked` that keeps the gate.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks: noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs::authz::Read;
use nest_rs::mcp::model::GetPromptResult;
use nest_rs::mcp::{McpError, mcp, tools};

mod posts {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "public_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[mcp]
#[derive(Clone)]
struct PostsTool;

#[tools]
impl PostsTool {
    /// Drafts a post.
    #[prompt]
    #[public(Read, posts::Entity)]
    async fn draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(Vec::new()))
    }
}

fn main() {}
