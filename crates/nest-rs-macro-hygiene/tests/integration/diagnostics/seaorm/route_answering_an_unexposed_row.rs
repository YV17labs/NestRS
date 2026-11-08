//! A route answering a row no `#[expose]` decorates says what crosses a wire,
//! whatever holds the row and whatever the row derives: a derive on a `Model` is
//! no opening.

mod secret {
    use sea_orm::entity::prelude::*;

    #[derive(
        Clone,
        Debug,
        PartialEq,
        DeriveEntityModel,
        serde::Serialize,
        nest_rs::http::schemars::JsonSchema,
    )]
    #[schemars(crate = "nest_rs::http::schemars")]
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

use nest_rs::http::{HttpError, Json, Response, controller, routes};

#[controller(path = "/secrets")]
pub struct SecretsController;

#[routes]
impl SecretsController {
    #[get("/one")]
    #[public]
    fn one(&self) -> Json<secret::Model> {
        loop {}
    }

    #[get("/all")]
    #[public]
    async fn all(&self) -> Result<Json<Vec<secret::Model>>, HttpError> {
        loop {}
    }

    #[get("/maybe")]
    #[public]
    fn maybe(&self) -> Json<Option<secret::Model>> {
        loop {}
    }

    #[get("/built")]
    #[public]
    #[api(response = secret::Model)]
    fn built(&self) -> Response {
        loop {}
    }
}

fn main() {}
