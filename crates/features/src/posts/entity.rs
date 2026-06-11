use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "Post", service = super::service::PostsService, graphql)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "post",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    pub author_id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub title: String,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub body: String,
    #[sea_orm(belongs_to, from = "org_id", to = "id")]
    pub org: HasOne<crate::orgs::Entity>,
    #[sea_orm(belongs_to, from = "author_id", to = "id")]
    pub author: HasOne<crate::users::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
