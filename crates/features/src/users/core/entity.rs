use nestrs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "User", complex)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "user",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
    #[sea_orm(unique)]
    #[expose(input(create, update), validate(email))]
    pub email: String,
    #[expose(skip)]
    pub role: String,
    #[expose(skip)]
    pub password_hash: Option<String>,
    #[sea_orm(belongs_to, from = "org_id", to = "id")]
    #[expose(skip)]
    pub org: HasOne<crate::orgs::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
