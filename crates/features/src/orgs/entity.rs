use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "Org", service = super::service::OrgsService)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "org",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
    #[sea_orm(has_many)]
    pub users: HasMany<crate::users::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
