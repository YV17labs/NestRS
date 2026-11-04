use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use nest_rs::authz::{Read, Update};
use nest_rs::events::EventBus;
use nest_rs::graphql::{crud, resolver};
use nest_rs::seaorm::Authorized;

use crate::authz::AuthzGuard;
use crate::posts::{
    Entity as PostEntity, Post, PostAuthorGuard, PostError, PostPublishedEvent, PostsService,
};

#[resolver]
#[use_guards(AuthzGuard)]
pub struct PostsResolver {
    #[inject]
    svc: Arc<PostsService>,
    #[inject]
    bus: Arc<EventBus>,
}

#[crud(service = PostsService, ops = [list, get])]
impl PostsResolver {
    #[mutation]
    #[authorize(Update, bind = PostsService)]
    #[use_guards(PostAuthorGuard)]
    async fn publish_post(&self, post: Authorized<Update, PostEntity>) -> Result<Post, PostError> {
        self.svc.publish(post.into_inner()).await
    }

    #[subscription]
    #[authorize(Read, PostEntity)]
    async fn post_published(&self) -> impl Stream<Item = Post> {
        self.bus
            .stream::<PostPublishedEvent>()
            .map(|event| event.post)
    }
}
