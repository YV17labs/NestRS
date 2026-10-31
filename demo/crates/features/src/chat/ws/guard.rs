use nest_rs::core::{Layer, UnitContext, async_trait, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor};
use nest_rs::ws::WsView;

use crate::Claims;

#[injectable]
#[derive(Layer)]
pub struct ModerationGuard;

#[async_trait]
impl Guard for ModerationGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Self::decide(&*cx.require::<Claims>()?)
    }

    fn target(&self) -> &'static str {
        crate::chat::TARGET
    }
}

impl GuardFor<WsView> for ModerationGuard {}

impl ModerationGuard {
    fn decide(caller: &Claims) -> Result<(), Denial> {
        match caller.sub.user_id() {
            Some(_) => Ok(()),
            None => {
                Err(Denial::forbidden("posting takes a token naming a user")
                    .with_reason("not_a_user"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::Subject;

    fn verdict(sub: Subject) -> Result<(), Denial> {
        let caller = Claims {
            sub,
            client_id: None,
            org_id: Uuid::now_v7(),
            roles: Vec::new(),
            scopes: Vec::new(),
        };
        ModerationGuard::decide(&caller)
    }

    #[test]
    fn a_user_posts() {
        assert!(verdict(Subject::User(Uuid::now_v7())).is_ok());
    }

    #[test]
    fn a_token_naming_no_user_is_refused() {
        let refused =
            verdict(Subject::Client("demo-service".into())).expect_err("a machine never posts");
        assert_eq!(refused.http_status(), 403);
        assert_eq!(refused.reason(), Some("not_a_user"));
    }
}
