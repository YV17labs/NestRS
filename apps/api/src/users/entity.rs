use nestrs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "User", complex)]
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[expose(skip)]
    pub org_id: Uuid,
    #[expose(input(create), validate(length(min = 1)))]
    pub name: String,
    #[sea_orm(unique)]
    #[expose(input(create), validate(email))]
    pub email: String,
}

impl ActiveModelBehavior for ActiveModel {}
