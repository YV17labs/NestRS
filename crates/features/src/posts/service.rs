use nest_rs_core::injectable;
use nest_rs_seaorm::{CreateModel, CrudService, Repo, ServiceError};
use sea_orm::ActiveModelTrait;
use sea_orm::Set;
use uuid::Uuid;

use super::entity::{CreatePostDto, Entity as Posts, Post, UpdatePostDto};

#[injectable]
#[derive(Default)]
pub struct PostsService;

impl CrudService for PostsService {
    type Entity = Posts;
    type Create = CreatePostDto;
    type Update = UpdatePostDto;

    fn soft_delete_column() -> Option<super::entity::Column> {
        Some(super::entity::Column::DeletedAt)
    }
}

impl PostsService {
    pub async fn create_in_org(
        &self,
        input: CreatePostDto,
        org_id: Uuid,
        author_id: Uuid,
    ) -> Result<Post, ServiceError> {
        let mut active = input.into_active_model();
        active.org_id = Set(org_id);
        active.author_id = Set(author_id);
        let model = active.insert(&Repo::<Posts>::conn()?).await?;
        tracing::info!(
            target: "features::posts",
            id = %model.id,
            %org_id,
            %author_id,
            "post created",
        );
        Ok(Post::from(&model))
    }
}
