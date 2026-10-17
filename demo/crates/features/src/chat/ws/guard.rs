use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor};
use nest_rs::ws::serde_json::Value;
use nest_rs::ws::{WsView, async_trait};

#[injectable]
#[derive(Default)]
pub struct ModerationGuard;

impl Layer for ModerationGuard {}

#[async_trait]
impl Guard for ModerationGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let Some(view) = cx.switch_to::<WsView>() else {
            tracing::error!(
                target: crate::chat::TARGET,
                reason = "not_a_message",
                "message denied: the guard checks WebSocket messages only",
            );
            return Err(Denial::internal(
                "ModerationGuard checks WebSocket messages only",
            ));
        };
        Self::decide(view.event(), view.data())
    }

    fn files_its_denials(&self) -> bool {
        true
    }
}

impl GuardFor<WsView> for ModerationGuard {}

impl ModerationGuard {
    fn decide(event: &str, data: Option<&Value>) -> Result<(), Denial> {
        match data
            .and_then(|data| data.get("author"))
            .and_then(Value::as_str)
        {
            Some("banned") => {
                tracing::warn!(
                    target: crate::chat::TARGET,
                    action = "post",
                    subject = event,
                    reason = "banned_author",
                    "message denied: author is not allowed to post",
                );
                Err(Denial::forbidden("author `banned` is not allowed to post"))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_a_banned_author() {
        let data = json!({ "author": "banned", "text": "x" });
        assert!(ModerationGuard::decide("message", Some(&data)).is_err());
    }

    #[test]
    fn allows_everyone_else() {
        let data = json!({ "author": "ada", "text": "x" });
        assert!(ModerationGuard::decide("message", Some(&data)).is_ok());
    }
}
