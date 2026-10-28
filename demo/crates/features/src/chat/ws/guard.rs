use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor};
use nest_rs::ws::{WsView, async_trait};

use crate::Claims;

#[injectable]
#[derive(Default)]
pub struct ModerationGuard;

impl Layer for ModerationGuard {}

#[async_trait]
impl Guard for ModerationGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Self::decide(cx.ambient::<Claims>().as_deref())
    }

    fn files_its_denials(&self) -> bool {
        true
    }
}

impl GuardFor<WsView> for ModerationGuard {}

impl ModerationGuard {
    fn decide(caller: Option<&Claims>) -> Result<(), Denial> {
        let Some(caller) = caller else {
            tracing::error!(
                target: crate::chat::TARGET,
                reason = "no_claims",
                "message denied: AuthnGuard did not run at the upgrade",
            );
            return Err(Denial::internal(
                "ModerationGuard requires AuthnGuard at the upgrade",
            ));
        };
        if caller.sub.user_id().is_some() {
            return Ok(());
        }
        tracing::warn!(
            target: crate::chat::TARGET,
            action = "post",
            org_id = %caller.org_id,
            reason = "not_a_user",
            "message denied",
        );
        Err(Denial::forbidden("posting takes a token naming a user"))
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
            exp: 0,
        };
        ModerationGuard::decide(Some(&caller))
    }

    #[test]
    fn a_user_posts() {
        assert!(verdict(Subject::User(Uuid::now_v7())).is_ok());
    }

    #[test]
    fn a_token_naming_no_user_is_refused() {
        let logs = nest_rs::testing::LogCapture::install();
        let refused =
            verdict(Subject::Client("demo-service".into())).expect_err("a machine never posts");
        assert_eq!(refused.http_status(), 403);

        let line = logs.expect_one(crate::chat::TARGET, "message denied");
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("reason").as_deref(), Some("not_a_user"));
    }

    #[test]
    fn a_message_no_principal_reached_fails_closed() {
        let logs = nest_rs::testing::LogCapture::install();
        let refused = ModerationGuard::decide(None).expect_err("an unwired upgrade admits no one");
        assert_eq!(refused.http_status(), 500);

        let line = logs.expect_one(
            crate::chat::TARGET,
            "message denied: AuthnGuard did not run at the upgrade",
        );
        assert_eq!(line.level, "error");
    }
}
