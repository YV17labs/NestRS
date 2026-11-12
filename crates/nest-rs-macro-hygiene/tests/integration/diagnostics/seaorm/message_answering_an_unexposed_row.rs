//! A message answering a row no `#[expose]` decorates says what crosses a wire,
//! as a route does, whatever holds the row and whatever the row derives.

mod secret {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "secrets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub password_hash: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::ws::{WsError, gateway, messages};

#[gateway(path = "/secrets")]
pub struct SecretsGateway;

#[messages]
impl SecretsGateway {
    #[subscribe_message("secret.one")]
    #[public]
    async fn one(&self) -> secret::Model {
        loop {}
    }

    #[subscribe_message("secret.all")]
    #[public]
    async fn all(&self) -> Result<Vec<secret::Model>, WsError> {
        loop {}
    }
}

fn main() {}
