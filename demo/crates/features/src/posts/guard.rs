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
#[derive(Layer)]
pub struct PostAuthorGuard;

#[async_trait]
impl Guard for PostAuthorGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let claims = cx.require::<Claims>()?;
        Self::attach_author(cx, &claims)
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let claims = cx
            .ambient::<Claims>()
            .ok_or_else(|| Self::user_required("no_claims"))?;
        Self::attach_author(cx, &claims)
    }

    fn denial_statuses(&self, _public: bool) -> &'static [u16] {
        &[403]
    }

    fn target(&self) -> &'static str {
        crate::posts::TARGET
    }
}

impl GuardFor<HttpView> for PostAuthorGuard {}

impl GuardFor<GraphqlView> for PostAuthorGuard {}

impl PostAuthorGuard {
    fn attach_author(cx: &mut UnitContext<'_>, claims: &Claims) -> Result<(), Denial> {
        let author = claims
            .sub
            .user_id()
            .ok_or_else(|| Self::user_required("not_a_user"))?;
        cx.attach(PostAuthor(author));
        Ok(())
    }

    fn user_required(reason: &'static str) -> Denial {
        Denial::forbidden("writing a post takes a token naming a user").with_reason(reason)
    }
}
