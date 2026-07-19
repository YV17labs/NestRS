use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::async_trait;
use poem::Request;
use uuid::Uuid;

use crate::Claims;

#[derive(Debug, Clone, Copy)]
pub struct PostAuthor(pub Uuid);

#[injectable]
#[derive(Default)]
pub struct PostAuthorGuard;

impl Layer for PostAuthorGuard {}

#[async_trait]
impl Guard for PostAuthorGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let author_id = {
            let claims = req.extensions().get::<Claims>().ok_or_else(|| {
                Denial::internal("PostAuthorGuard requires AuthGuard to run first")
            })?;
            let Some(sub) = claims.sub else {
                tracing::warn!(
                    target: "features::posts",
                    org_id = %claims.org_id,
                    "post create denied: token carries no subject",
                );
                return Err(Denial::forbidden(
                    "a bearer token with a subject is required to create a post",
                ));
            };
            sub
        };
        req.extensions_mut().insert(PostAuthor(author_id));
        Ok(())
    }
}
