//! A derive above `#[expose]` has expanded before the decorator reads the item, so
//! no `#[derive]` it refuses is left to read: the bounded impls overlap what it
//! wrote instead, whichever of the two it is.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[derive(serde::Serialize, serde::Deserialize)]
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

fn main() {}
