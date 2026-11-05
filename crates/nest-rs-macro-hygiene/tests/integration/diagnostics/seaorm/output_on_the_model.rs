//! `#[output]` and `#[input]` write a DTO's serde, which on a row is every column's:
//! each overlaps what `#[expose]` emits there, whichever of the two is written first.

mod post {
    use nest_rs::core::output;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[output]
    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod note {
    use nest_rs::core::output;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[output]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod draft {
    use nest_rs::core::input;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[input]
    #[expose(name = "Draft")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "drafts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn main() {}
