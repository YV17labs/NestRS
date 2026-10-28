//! A hand-written `Authorize<Action, Entity>` is the public-reads spelling,
//! beside `#[public]`. On a route declaring no posture it would be one no
//! attribute shows, so the parameter is refused, naming `#[authorize]`.

use nest_rs::authz::Read;
use nest_rs::authz::http::Authorize;
use nest_rs::http::{controller, routes};

mod posts {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "gated_by_type_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    async fn list(&self, _authz: Authorize<Read, posts::Entity>) -> String {
        String::new()
    }
}

fn main() {}
