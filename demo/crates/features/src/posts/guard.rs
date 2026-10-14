use nest_rs::core::{AmbientValue, Layer, UnitContext, injectable};
use nest_rs::graphql::GraphqlView;
use nest_rs::guards::{Denial, Guard, GuardFor};
use nest_rs::http::{HttpView, async_trait};
use uuid::Uuid;

use crate::Claims;

#[derive(Debug, Clone, Copy)]
pub struct PostAuthor(pub Uuid);

impl AmbientValue for PostAuthor {}

#[injectable]
#[derive(Default)]
pub struct PostAuthorGuard;

impl Layer for PostAuthorGuard {}

#[async_trait]
impl Guard for PostAuthorGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let claims = cx
            .ambient::<Claims>()
            .ok_or_else(|| Denial::internal("PostAuthorGuard requires AuthnGuard to run first"))?;
        let author = Self::author(&claims)?;
        cx.attach(PostAuthor(author));
        Ok(())
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let Some(claims) = cx.ambient::<Claims>() else {
            tracing::warn!(
                target: crate::posts::TARGET,
                reason = "no_claims",
                "post write denied",
            );
            return Err(Self::subject_required());
        };
        Self::author(&claims).map(|_| ())
    }
}

impl GuardFor<HttpView> for PostAuthorGuard {}

impl GuardFor<GraphqlView> for PostAuthorGuard {}

impl PostAuthorGuard {
    fn author(claims: &Claims) -> Result<Uuid, Denial> {
        claims.sub.ok_or_else(|| {
            tracing::warn!(
                target: crate::posts::TARGET,
                org_id = %claims.org_id,
                reason = "no_subject",
                "post write denied",
            );
            Self::subject_required()
        })
    }

    fn subject_required() -> Denial {
        Denial::forbidden("a bearer token with a subject is required to write posts")
    }
}
