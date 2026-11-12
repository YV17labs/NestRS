use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::events::EventBus;
use nest_rs::queue::Jobs;
use nest_rs::seaorm::{
    Creatable, CrudService, Deletable, Repo, RepoError, ServiceError, Updatable,
};
use sea_orm::{IntoActiveModel, Set};

use super::entities::post::{CreatePost, Entity as Posts, Model, Post, PostStatus, UpdatePost};
use super::entities::publication;
use super::error::PostError;
use super::event::PostPublishedEvent;
use super::store::PublicationStore;
use crate::notifications::{NotifyCommand, NotifyQueue};

#[injectable]
pub struct PostsService {
    #[inject]
    bus: Arc<EventBus>,
    #[inject]
    jobs: Arc<Jobs>,
    #[inject]
    publications: Arc<PublicationStore>,
}

impl CrudService for PostsService {
    type Entity = Posts;
}

impl Creatable for PostsService {
    type Create = CreatePost;
}

impl Updatable for PostsService {
    type Update = UpdatePost;
}

impl Deletable for PostsService {}

impl PostsService {
    pub async fn publish(&self, model: Model) -> Result<Post, PostError> {
        if model.status == PostStatus::Published {
            return Err(PostError::AlreadyPublished { id: model.id });
        }

        let post_id = model.id;
        let org_id = model.org_id;

        let mut active = model.into_active_model();
        active.status = Set(PostStatus::Published);
        let published = Repo::<Posts>::update(active)
            .await
            .map_err(ServiceError::from)?;

        let notification = NotifyCommand {
            org_id,
            message: format!("Post \"{}\" was published", published.title),
        };
        let receipt = self.jobs.push(NotifyQueue, notification).await?;

        self.publications
            .create(publication::ActiveModel {
                post_id: Set(post_id),
                ..Default::default()
            })
            .await
            .map_err(|err| match err {
                RepoError::UniqueViolation(_) => PostError::AlreadyPublished { id: post_id },
                other => PostError::Service(other.into()),
            })?;

        tracing::debug!(
            target: crate::posts::TARGET,
            id = %post_id,
            %org_id,
            job_id = %receipt.id(),
            "post published",
        );
        let post = Post::from(&published);
        self.bus.emit(PostPublishedEvent { post: published }).await;
        Ok(post)
    }
}
