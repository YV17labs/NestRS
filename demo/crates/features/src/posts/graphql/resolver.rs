use std::sync::Arc;

use async_graphql::futures_util::stream::Stream;
use async_graphql::{Context, Error, Result};
use nest_rs::authz::{Read, Update};
use nest_rs::graphql::{crud, problem_error, resolver};
use nest_rs::seaorm::Authorized;

use crate::Claims;
use crate::authz::AuthzGuard;
use crate::posts::{Entity as PostEntity, Post, PostAuthorGuard, PostFeed, PostsService};

#[resolver]
#[use_guards(AuthzGuard)]
pub struct PostsResolver {
    #[inject]
    svc: Arc<PostsService>,
    #[inject]
    feed: Arc<PostFeed>,
}

#[crud(
    service = svc,
    entity = PostEntity,
    output = Post,
    ops = [list, get],
)]
impl PostsResolver {
    #[mutation]
    #[authorize(Update, bind = PostsService)]
    #[use_guards(PostAuthorGuard)]
    async fn publish_post(
        &self,
        ctx: &Context<'_>,
        post: Authorized<Update, PostEntity>,
    ) -> Result<Post> {
        let actor_id = ctx.data::<Claims>()?.sub.user_id().ok_or_else(|| {
            async_graphql::Error::new("PostAuthorGuard must run before publish_post")
        })?;
        self.svc
            .publish(post.into_inner(), actor_id)
            .await
            .map_err(|error| problem_error(&error))
    }

    #[subscription]
    #[authorize(Read, PostEntity)]
    async fn post_published(&self) -> Result<impl Stream<Item = Post>, Error> {
        Ok(self.feed.subscribe())
    }
}
