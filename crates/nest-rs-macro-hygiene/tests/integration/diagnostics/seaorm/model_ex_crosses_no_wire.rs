//! `#[sea_orm::model]`'s `ModelEx` holds every column beside its relations, so what
//! holds at the `Model` holds at it: writing one says what crosses a wire instead.

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
}

fn written(row: &post::ModelEx) {
    let _ = serde_json::to_string(row);
    let _ = serde_json::from_str::<post::ModelEx>("{}");
}

fn main() {}
