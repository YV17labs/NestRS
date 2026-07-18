use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_events::EventBus;
use nest_rs_seaorm::{
    Creatable, CreateModel, CrudService, Deletable, Repo, ServiceError, Updatable,
};
use sea_orm::ActiveModelTrait;
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
        // A freshly created post is a draft; publishing is a deliberate second
        // step (`publish`), so creation never notifies subscribers.
        active.status = Set(PostStatus::Draft);
        let model = active.insert(&Repo::<Posts>::conn()?).await?;
        tracing::debug!(
            target: "features::posts",
            id = %model.id,
            %org_id,
            %author_id,
            "post created",
        );
        Ok(Post::from(&model))
    }

    /// The publish precondition: a post may be published only from `Draft`.
    ///
    /// The state rule lives in the service — not the HTTP handler — so the
    /// decision stays where business logic belongs; the `publish` route maps the
    /// returned [`PostError`] to RFC 9457 problem+json via `PostProblemFilter`.
    pub fn ensure_unpublished(&self, model: &Model) -> Result<(), PostError> {
        if model.status == PostStatus::Published {
            return Err(PostError::AlreadyPublished { id: model.id });
        }
        Ok(())
    }

    /// Transition a loaded post to `Published` and announce the fact.
    ///
    /// The row must already be authorized for `Update` by the caller's ability
    /// (the resolver binds it through `CrudService::access`); [`Repo::update`]
    /// re-applies that scope, so a row outside the caller's reach is never
    /// touched. Publishing — not creation — is what emits [`PostPublishedEvent`]:
    /// fire-and-forget, so a listener panic never fails the transition. See the
    /// `notifications` slice.
    pub async fn publish(&self, model: Model) -> Result<Post, ServiceError> {
        let post_id = model.id;
        let org_id = model.org_id;
        let title = model.title.clone();

        let mut active = model.into_active_model();
        active.status = Set(PostStatus::Published);
        let published = Repo::<Posts>::update(active).await?;

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
