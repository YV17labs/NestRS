//! An `Authorized<Action, E>` parameter carries the row a `bind = Service`
//! loads, never the posture: beside an `#[authorize]` that binds nothing, it is
//! refused by name rather than as an argument type async-graphql cannot read.

use nest_rs::authz::Update;
use nest_rs::graphql::{operations, resolver};
use nest_rs::seaorm::Authorized;

mod users {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "unbound_resolver_users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[resolver]
struct UsersResolver;

#[operations]
impl UsersResolver {
    #[mutation]
    #[authorize(Update, users::Entity)]
    async fn archive_user(&self, user: Authorized<Update, users::Entity>) -> bool {
        let _ = user;
        true
    }
}

fn main() {}
