use nest_rs_core::injectable;
use nest_rs_seaorm::{CreateModel, CrudService, Repo, ServiceError};
use sea_orm::ActiveModelTrait;
use sea_orm::Set;
use uuid::Uuid;

use super::entity::{CreatePostInput, Entity as Posts, Post, UpdatePostInput};

#[injectable]
#[derive(Default)]
pub struct PostsService;

impl CrudService for PostsService {
    type Entity = Posts;
    type Create = CreatePostInput;
    type Update = UpdatePostInput;
}

impl PostsService {
    pub async fn create_in_org(
        &self,
        input: CreatePostInput,
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
