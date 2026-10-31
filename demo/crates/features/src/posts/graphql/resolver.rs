use std::sync::Arc;

use async_graphql::Error;
use async_graphql::futures_util::stream::Stream;
use nest_rs::authz::{Read, Update};
use nest_rs::core::Ambient;
use nest_rs::graphql::{crud, resolver};
use nest_rs::seaorm::Authorized;

use crate::authz::AuthzGuard;
use crate::posts::{
    Entity as PostEntity, Post, PostAuthor, PostAuthorGuard, PostError, PostFeed, PostsService,
};

#[resolver]
#[use_guards(AuthzGuard)]
pub struct PostsResolver {
    #[inject]
    svc: Arc<PostsService>,
    #[inject]
    feed: Arc<PostFeed>,
}

#[crud(service = svc, ops = [list, get])]
impl PostsResolver {
    #[mutation]
    #[authorize(Update, bind = PostsService)]
    #[use_guards(PostAuthorGuard)]
    async fn publish_post(
        &self,
        post: Authorized<Update, PostEntity>,
        actor: Ambient<PostAuthor>,
    ) -> Result<Post, PostError> {
        let PostAuthor(actor_id) = *actor;
        self.svc.publish(post.into_inner(), actor_id).await
    }

    #[subscription]
    #[authorize(Read, PostEntity)]
    async fn post_published(&self) -> Result<impl Stream<Item = Post>, Error> {
        Ok(self.feed.subscribe())
    }
}
