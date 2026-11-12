//! A GraphQL object is a wire of its own: an async-graphql derive above `#[expose]`,
//! or `#[Object]` on the row, writes every column, and the bounded impl
//! overlaps it as it overlaps serde's, whether or not the entity declares `graphql`.

mod post {
    use async_graphql::SimpleObject;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[derive(SimpleObject)]
    #[expose(name = "Post", graphql)]
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
    use async_graphql::Object;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note", graphql)]
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

    #[Object]
    impl Model {
        async fn author_email(&self) -> &str {
            &self.author_email
        }
    }
}

mod draft {
    use async_graphql::InputObject;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[derive(InputObject)]
    #[expose(name = "Draft", graphql)]
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

mod keyless {
    use async_graphql::SimpleObject;
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[derive(SimpleObject)]
    #[expose(name = "Keyless")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "keyless")]
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
