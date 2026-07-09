use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_events::EventBus;
use nest_rs_seaorm::{CreateModel, Creatable, CrudService, Deletable, Repo, ServiceError, Updatable};
use sea_orm::ActiveModelTrait;
use sea_orm::Set;
use uuid::Uuid;

use super::entity::{CreatePost, Entity as Posts, Post, UpdatePost};
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
        let model = active.insert(&Repo::<Posts>::conn()?).await?;
        tracing::info!(
            target: "features::posts",
            id = %model.id,
            %org_id,
            %author_id,
            "post created",
        );
        // Publish the fact. Fire-and-forget: emit does not await delivery, and
        // a listener panic never fails the create. See the `notifications` slice.
        self.bus
            .emit(PostPublishedEvent {
                post_id: model.id,
                org_id,
                title: model.title.clone(),
            })
            .await;
        Ok(Post::from(&model))
    }
}
