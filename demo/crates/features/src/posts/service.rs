use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_events::EventBus;
use nest_rs_seaorm::{
    Creatable, CreateModel, CrudService, Deletable, Repo, ServiceError, Updatable,
};
use sea_orm::IntoActiveModel;
use sea_orm::Set;
use uuid::Uuid;

use super::entity::{CreatePost, Entity as Posts, Model, Post, PostStatus, UpdatePost};
use super::error::PostError;
use super::event::PostPublishedEvent;

#[injectable]
pub struct PostsService {
    #[inject]
    bus: Arc<EventBus>,
}

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
        active.status = Set(PostStatus::Draft);
        let model = self.create_from_active(active).await?;
        tracing::debug!(
            target: "features::posts",
            id = %model.id,
            %org_id,
            %author_id,
            "post created",
        );
        Ok(Post::from(&model))
    }

    /// Publish a draft. The "already published" invariant lives here — the one
    /// place every transport reaches — so republishing is a conflict on HTTP,
    /// GraphQL and MCP alike, never a duplicate `PostPublishedEvent`.
    pub async fn publish(&self, model: Model) -> Result<Post, PostError> {
        if model.status == PostStatus::Published {
            return Err(PostError::AlreadyPublished { id: model.id });
        }

        let post_id = model.id;
        let org_id = model.org_id;
        let title = model.title.clone();

        let mut active = model.into_active_model();
        active.status = Set(PostStatus::Published);
        let published = Repo::<Posts>::update(active)
            .await
            .map_err(ServiceError::from)?;

        tracing::debug!(
            target: "features::posts",
            id = %post_id,
            %org_id,
            "post published",
        );
        self.bus
            .emit(PostPublishedEvent {
                post_id,
                org_id,
                title,
            })
            .await;
        Ok(Post::from(&published))
    }
}
