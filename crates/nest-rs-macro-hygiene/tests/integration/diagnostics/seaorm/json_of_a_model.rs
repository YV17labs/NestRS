//! Writing a row, whatever holds it, says what crosses a wire instead of asking
//! for the derive `#[expose]` refuses.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

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

#[derive(serde::Serialize)]
struct Page {
    rows: Vec<post::Model>,
}

fn written(row: &post::Model, page: &Page) {
    let _ = serde_json::to_string(row);
    let _ = serde_json::to_value(vec![row]);
    let _ = serde_json::to_string(page);
}

fn main() {}
