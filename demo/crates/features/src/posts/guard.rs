use nest_rs::core::{Layer, UnitContext, UnitView, async_trait, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};

use crate::Claims;

#[injectable]
#[derive(Layer)]
pub struct PostAuthorGuard;

#[async_trait]
impl Guard for PostAuthorGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let claims = cx.require::<Claims>()?;
        match claims.sub.user_id() {
            Some(_) => Ok(()),
            None => Err(
                Denial::forbidden("writing a post takes a token naming a user")
                    .with_reason("not_a_user"),
            ),
        }
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }

    fn expected_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<Claims>())
    }

    fn target(&self) -> &'static str {
        crate::posts::TARGET
    }
}

impl<V: UnitView> GuardFor<V> for PostAuthorGuard {}
