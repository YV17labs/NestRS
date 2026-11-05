//! A schema derived on a row publishes every column's name and type, hidden ones
//! included, in a document no mask shapes: refused at the derive `#[expose]` reads,
//! and above it, where the bounded impl overlaps what it wrote.

mod post {
    use nest_rs::http::schemars::JsonSchema;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, JsonSchema)]
    #[schemars(crate = "nest_rs::http::schemars")]
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
    use nest_rs::http::schemars::JsonSchema;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[derive(JsonSchema)]
    #[schemars(crate = "nest_rs::http::schemars")]
    #[expose(name = "Note")]
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

fn main() {}
