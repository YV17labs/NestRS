use sea_orm::entity::prelude::*;

/// The publish audit log — a plain, internal secondary entity of the posts
/// feature (no `#[expose]`, no wire surface). `PostsService::publish` inserts
/// one row per publish through `Repo`, in the same request transaction as the
/// status update, so the two writes commit or roll back together.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "post_publication")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub post_id: Uuid,
    pub actor_id: Uuid,
    pub published_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
