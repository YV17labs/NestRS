//! `#[sea_orm::model]`'s `ModelEx` holds every column beside its relations, so an
//! impl written by hand on it is refused as one on the `Model` is.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        pub author_email: String,
    }

    impl ActiveModelBehavior for ActiveModel {}

    impl serde::Serialize for ModelEx {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&self.author_email)
        }
    }
}

fn main() {}
