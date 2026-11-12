//! The refusal of `Authorize<Action, Subject>` as a parameter is by its type, so
//! a renamed import meets the same words a spelling check would have missed.

use nest_rs::authz::Read;
use nest_rs::authz::http::Authorize as Gate;
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
    async fn list(&self, _authz: Gate<Read, posts::Entity>) -> String {
        String::new()
    }
}

fn main() {}
