use std::sync::Arc;

use async_graphql::{Context, Error, Result};
use nest_rs_authz::{Action, Update};
use nest_rs_graphql::{crud, resolver};
use nest_rs_seaorm::{Access, CrudService};
use uuid::Uuid;

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::posts::{Entity as PostEntity, Post, PostsService};

#[resolver]
#[use_guards(AuthGuard, AuthzGuard)]
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
    async fn publish_post(&self, _ctx: &Context<'_>, id: String) -> Result<Option<Post>> {
        let post_id = Uuid::parse_str(&id).map_err(|e| Error::new(e.to_string()))?;
        match CrudService::access(&*self.svc, Action::Update, post_id).await? {
            Access::Found(model) => Ok(Some(self.svc.publish(model).await?)),
            Access::Denied => Err(Error::new("forbidden")),
            Access::Missing => Ok(None),
        }
    }
}
