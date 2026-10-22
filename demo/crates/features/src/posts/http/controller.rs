use std::sync::Arc;

use nest_rs::authz::{Create, Update};
use nest_rs::http::{
    Ctx, IntoResponse, Json, Request, Response, Result, Valid, caller_path, controller, crud,
    set_created_location,
};
use nest_rs::seaorm::Bind;

use super::exception_filter::PostProblemFilter;
use super::interceptor::PostAuditInterceptor;
use crate::Claims;
use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::posts::{
    CreatePost, Entity as PostEntity, Post, PostAuthor, PostAuthorGuard, PostsService, UpdatePost,
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
)]
impl PostsController {
    #[post("/")]
    #[http_code(201)]
    #[authorize(Create, PostEntity)]
    #[use_guards(PostAuthorGuard)]
    #[api(
        summary = "Create a post in the caller's org",
        description = "Requires a bearer JWT with a subject. The org and author are taken from \
                       the token, never the body.",
        response = Post
    )]
    async fn create(
        &self,
        req: &Request,
        auth: Ctx<Claims>,
        author: Ctx<PostAuthor>,
        body: Valid<Json<CreatePost>>,
    ) -> Result<Response> {
        let PostAuthor(author_id) = *author;
        let post = self
            .svc
            .create_in_org(body.into_inner(), auth.org_id, author_id)
            .await?;
        let mut res = Json(&post).into_response();
        set_created_location(&mut res, caller_path(req), &post.id);
        Ok(res)
    }

    #[post("/{id}/publish")]
    #[authorize(Update, PostEntity)]
    #[use_guards(PostAuthorGuard)]
    #[use_exception_filters(PostProblemFilter)]
    #[api(
        summary = "Publish a draft post",
        description = "Transitions a draft to published and writes a publication audit row in the \
                       same transaction. The id is bound to the loaded, `Update`-authorized post \
                       through the service. Re-publishing an already published post returns RFC \
                       9457 `application/problem+json` (409)."
    )]
    async fn publish(
        &self,
        post: Bind<Update, PostsService>,
        actor: Ctx<PostAuthor>,
    ) -> Result<Json<Post>> {
        let PostAuthor(actor_id) = *actor;
        Ok(Json(self.svc.publish(post.into_inner(), actor_id).await?))
    }
}
