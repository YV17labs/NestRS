use std::sync::Arc;

use nest_rs_authz::Create;
use nest_rs_authz::http::Authorize;
use nest_rs_http::{Ctx, Valid, controller, crud};
use poem::Result;
use poem::http::StatusCode;
use poem::web::Json;

use crate::Claims;
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::posts::{CreatePostDto, Entity as PostEntity, Post, PostsService, UpdatePostDto};

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
    create = CreatePostDto,
    update = UpdatePostDto,
)]
impl PostsController {
    #[post("/")]
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
        body: Valid<Json<CreatePostDto>>,
    ) -> Result<Json<Post>> {
        let author_id = auth.sub.ok_or_else(|| {
            poem::Error::from_string(
                "a bearer token with a subject is required to create a post",
                StatusCode::FORBIDDEN,
            )
        })?;
        Ok(Json(
            self.svc
                .create_in_org(body.into_inner(), auth.org_id, author_id)
                .await?,
        ))
    }
}
