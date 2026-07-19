use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    EnumIter,
    DeriveActiveEnum,
    async_graphql::Enum,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "lowercase")]
pub enum PostStatus {
    #[sea_orm(string_value = "draft")]
    Draft,
    #[sea_orm(string_value = "published")]
    Published,
}

#[expose(
    name = "Post",
    service = super::service::PostsService,
    graphql,
    soft_delete,
    timestamps
)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "post",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[expose]
    pub author_id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub title: String,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub body: String,
    #[expose]
    pub status: PostStatus,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
    #[expose]
    pub updated_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(belongs_to, from = "org_id", to = "id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
    #[sea_orm(belongs_to, from = "author_id", to = "id")]
    #[expose]
    pub author: HasOne<crate::users::Entity>,
}
