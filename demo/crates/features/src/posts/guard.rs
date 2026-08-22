use nest_rs::core::{Layer, injectable};
use nest_rs::graphql::GraphqlOperationContext;
use nest_rs::guards::{Denial, GraphqlGuard, Guard, HttpGuard};
use nest_rs::http::async_trait;
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
                Denial::internal("PostAuthorGuard requires AuthnGuard to run first")
            })?;
            let Some(sub) = claims.sub else {
                tracing::warn!(
                    target: "features::posts",
                    org_id = %claims.org_id,
                    reason = "no_subject",
                    "post write denied",
                );
                return Err(Denial::forbidden(
                    "a bearer token with a subject is required to write posts",
                ));
            };
            sub
        };
        req.extensions_mut().insert(PostAuthor(author_id));
        Ok(())
    }

    async fn check_graphql(&self, op: &GraphqlOperationContext<'_>) -> Result<(), Denial> {
        match op.data_opt::<Claims>() {
            Some(claims) if claims.sub.is_some() => Ok(()),
            Some(claims) => {
                tracing::warn!(
                    target: "features::posts",
                    org_id = %claims.org_id,
                    "post write denied: token carries no subject",
                );
                Err(Denial::forbidden(
                    "a bearer token with a subject is required to write posts",
                ))
            }
            None => {
                tracing::warn!(
                    target: "features::posts",
                    reason = "no_claims",
                    "post write denied",
                );
                Err(Denial::forbidden(
                    "a bearer token with a subject is required to write posts",
                ))
            }
        }
    }
}

impl GraphqlGuard for PostAuthorGuard {}

impl HttpGuard for PostAuthorGuard {}
