use std::sync::Arc;

use nest_rs_authz::Create;
use nest_rs_authz::http::Authorize;
use nest_rs_http::{Ctx, Valid, controller, crud};
use poem::Result;
use poem::web::Json;

use super::guard::{PostAuthor, PostAuthorGuard};
use crate::Claims;
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::posts::{CreatePost, Entity as PostEntity, Post, PostsService, UpdatePost};

#[controller(path = "/posts")]
#[use_guards(AuthGuard, AuthzGuard)]
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
        // `PostAuthorGuard` already verified the token carries a subject and
        // attached it as `PostAuthor`; the org comes from the same token.
        let PostAuthor(author_id) = *author;
        Ok(Json(
            self.svc
                .create_in_org(body.into_inner(), auth.org_id, author_id)
                .await?,
        ))
    }
}
