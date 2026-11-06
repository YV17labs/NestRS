//! Nothing would hold the row the mask cuts behind a `Weak`, so the row would ship
//! what a grant withholds.

mod writer {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Writer", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "writers")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::graphql::async_graphql::SimpleObject;
use nest_rs::graphql::{operations, resolver};

#[derive(SimpleObject)]
#[graphql(complex, crate = "nest_rs::graphql::async_graphql")]
pub struct Reading {
    id: i32,
}

#[resolver]
pub struct ReadingResolver;

#[operations]
impl ReadingResolver {
    #[query]
    #[public]
    async fn reading(&self) -> Reading {
        Reading { id: 1 }
    }

    #[field_resolver]
    async fn lead(&self, _parent: &Reading) -> std::sync::Weak<writer::Writer> {
        loop {}
    }
}

fn main() {}
