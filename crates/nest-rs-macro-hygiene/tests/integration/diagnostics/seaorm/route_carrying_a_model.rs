//! A route answering a row, or reading one off its body, says what crosses a wire:
//! a reply answers the exposed type and a write takes an input.

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

use nest_rs::http::{Json, controller, routes};

#[controller(path = "/posts")]
pub struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    #[public]
    fn answered(&self) -> Json<post::Model> {
        loop {}
    }

    #[post("/")]
    #[public]
    fn read(&self, Json(_row): Json<post::Model>) {}
}

fn main() {}
