use std::sync::Arc;

use nest_rs::authz::Update;
use nest_rs::core::Ctx;
use nest_rs::http::{Json, Result, controller, crud};
use nest_rs::seaorm::Authorized;

use super::interceptor::PostAuditInterceptor;
use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::posts::{
    CreatePost, Entity as PostEntity, Post, PostAuthor, PostAuthorGuard, PostError, PostsService,
    UpdatePost,
};

#[controller(path = "/posts")]
#[use_guards(AuthnGuard, AuthzGuard)]
#[use_interceptors(PostAuditInterceptor)]
pub struct PostsController {
    #[inject]
    svc: Arc<PostsService>,
}

#[crud(
    service = svc,
    entity = PostEntity,
    output = Post,
    create = CreatePost,
    update = UpdatePost,
    api(create(
        summary = "Create a post in the caller's org",
        description = "Requires a bearer JWT naming a user. The org and author are taken from \
                       the token, never the body."
    )),
)]
impl PostsController {
    #[post("/{id}/publish")]
    #[authorize(Update, bind = PostsService)]
    #[use_guards(PostAuthorGuard)]
    #[api(
        summary = "Publish a draft post",
        description = "Transitions a draft to published and writes a publication audit row in the \
                       same transaction. The id loads the `Update`-authorized post through the \
                       service. Re-publishing an already published post returns RFC 9457 \
                       `application/problem+json` (409)."
    )]
    async fn publish(
        &self,
        post: Authorized<Update, PostEntity>,
        actor: Ctx<PostAuthor>,
    ) -> Result<Json<Post>, PostError> {
        let PostAuthor(actor_id) = *actor;
        Ok(Json(self.svc.publish(post.into_inner(), actor_id).await?))
    }
}
