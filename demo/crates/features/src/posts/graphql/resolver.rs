use std::sync::Arc;

use async_graphql::futures_util::stream::Stream;
use async_graphql::{Context, Error, Result};
use nest_rs::authz::{Read, Update};
use nest_rs::graphql::{crud, resolver};
use nest_rs::seaorm::graphql::bind;

use crate::Claims;
use crate::app_authz::AppAuthzGuard;
use crate::posts::{Entity as PostEntity, Post, PostAuthorGuard, PostFeed, PostsService};

#[resolver]
#[use_guards(AppAuthzGuard)]
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
    #[authorize(Update, PostEntity)]
    #[use_guards(PostAuthorGuard)]
    async fn publish_post(&self, ctx: &Context<'_>, id: String) -> Result<Option<Post>> {
        let actor_id = ctx.data::<Claims>()?.sub.ok_or_else(|| {
            async_graphql::Error::new("PostAuthorGuard must run before publish_post")
        })?;
        match bind::<Update, PostsService>(ctx, &id).await? {
            Some(model) => Ok(Some(self.svc.publish(model, actor_id).await?)),
            None => Ok(None),
        }
    }

    #[subscription]
    #[authorize(Read, PostEntity)]
    async fn post_published(&self) -> Result<impl Stream<Item = Post>, Error> {
        Ok(self.feed.subscribe())
    }
}
