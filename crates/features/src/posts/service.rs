use nest_rs_core::injectable;
use nest_rs_seaorm::{CreateModel, Creatable, CrudService, Deletable, Repo, ServiceError, Updatable};
use sea_orm::ActiveModelTrait;
use sea_orm::Set;
use uuid::Uuid;

use super::entity::{CreatePost, Entity as Posts, Post, UpdatePost};

#[injectable]
#[derive(Default)]
pub struct PostsService;

impl CrudService for PostsService {
    type Entity = Posts;

    fn soft_delete_column() -> Option<super::entity::Column> {
        Some(super::entity::Column::DeletedAt)
    }
}

impl Creatable for PostsService {
    type Create = CreatePost;
}

impl Updatable for PostsService {
    type Update = UpdatePost;
}

impl Deletable for PostsService {}

impl PostsService {
    pub async fn create_in_org(
        &self,
        input: CreatePost,
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
