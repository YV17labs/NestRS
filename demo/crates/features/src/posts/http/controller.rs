use std::sync::Arc;

use nest_rs_authz::http::Authorize;
use nest_rs_authz::{Create, Update};
use nest_rs_http::{Ctx, Valid, controller, crud};
use nest_rs_seaorm::Bind;
use poem::Result;
use poem::web::Json;

use super::exception_filter::PostProblemFilter;
use super::guard::{PostAuthor, PostAuthorGuard};
use super::interceptor::PostAuditInterceptor;
use crate::Claims;
use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::posts::{CreatePost, Entity as PostEntity, Post, PostsService, UpdatePost};

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
    #[use_guards(PostAuthorGuard)]
    #[api(
        summary = "Create a post in the caller's org",
        description = "Requires a bearer JWT with a subject. The org and author are taken from \
                       the token, never the body.",
        tags("Post")
    )]
    async fn create(
        &self,
        _authz: Authorize<Create, PostEntity>,
        auth: Ctx<Claims>,
        author: Ctx<PostAuthor>,
        body: Valid<Json<CreatePost>>,
    ) -> Result<Json<Post>> {
        let PostAuthor(author_id) = *author;
        Ok(Json(
            self.svc
                .create_in_org(body.into_inner(), auth.org_id, author_id)
                .await?,
        ))
    }

    #[post("/:id/publish")]
    #[use_exception_filters(PostProblemFilter)]
    #[api(
        summary = "Publish a draft post",
        description = "Transitions a draft to published. The id is bound to the loaded, \
                       `Update`-authorized post through the service. Re-publishing an already \
                       published post returns RFC 9457 `application/problem+json` (409).",
        tags("Post")
    )]
    async fn publish(
        &self,
        _authz: Authorize<Update, PostEntity>,
        post: Bind<PostsService, Update>,
    ) -> Result<Json<Post>> {
        Ok(Json(self.svc.publish(post.into_inner()).await?))
    }
}
