//! An `Authorized<Action, E>` parameter carries the row a `bind = Service`
//! loads, never the posture: written beside an `#[authorize]` that binds
//! nothing, it is refused by name rather than as an extractor rustc cannot
//! find.

use nest_rs::authz::Update;
use nest_rs::http::{controller, routes};
use nest_rs::seaorm::Authorized;

mod users {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "unbound_users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[post("/{id}/archive")]
    #[authorize(Update, users::Entity)]
    async fn archive(&self, user: Authorized<Update, users::Entity>) -> String {
        user.id.to_string()
    }
}

fn main() {}
