//! `via` on a plain column means nothing at all — the second half of the
//! wrong-shape refusal, with the hint that fits a field declaring no relation.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post", service = PostsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(via = "author_id")]
    pub title: String,
}

fn main() {}
