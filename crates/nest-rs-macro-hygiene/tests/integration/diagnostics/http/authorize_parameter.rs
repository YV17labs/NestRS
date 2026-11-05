//! The parameter that used to spell a public read: a route's posture is a
//! decorator, so `Authorize<Action, Subject>` is refused as a parameter, by its
//! type, naming `#[public(Action, Subject)]`.

use nest_rs::authz::Read;
use nest_rs::authz::http::Authorize;
use nest_rs::http::{controller, routes};

mod posts {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "public_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
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
    #[public]
    async fn list(&self, _authz: Authorize<Read, posts::Entity>) -> String {
        String::new()
    }
}

fn main() {}
