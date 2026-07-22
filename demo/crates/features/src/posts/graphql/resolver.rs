use std::sync::Arc;

use async_graphql::{Context, Result};
use nest_rs_authz::Update;
use nest_rs_graphql::{crud, resolver};
use nest_rs_seaorm::graphql::bind;

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::posts::{Entity as PostEntity, Post, PostsService};

#[resolver]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct PostsResolver {
    #[inject]
    svc: Arc<PostsService>,
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
    async fn publish_post(&self, ctx: &Context<'_>, id: String) -> Result<Option<Post>> {
        match bind::<PostsService, Update>(ctx, &id).await? {
            Some(model) => Ok(Some(self.svc.publish(model).await?)),
            None => Ok(None),
        }
    }
}
