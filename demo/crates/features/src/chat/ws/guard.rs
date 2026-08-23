use nest_rs::core::{Layer, injectable};
use nest_rs::guards::{Denial, Guard, WsGuard};
use nest_rs::ws::serde_json::Value;
use nest_rs::ws::{WsClient, async_trait};

#[injectable]
#[derive(Default)]
pub struct ModerationGuard;

impl Layer for ModerationGuard {}

#[async_trait]
impl Guard for ModerationGuard {
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        event: &str,
        data: &Value,
    ) -> Result<(), Denial> {
        match data.get("author").and_then(Value::as_str) {
            Some(author @ "banned") => {
                tracing::warn!(
                    target: "features::chat",
                    action = "post",
                    subject = event,
                    author,
                    "message denied: author is not allowed to post",
                );
                Err(Denial::forbidden("author `banned` is not allowed to post"))
            }
            _ => Ok(()),
        }
    }
}

impl WsGuard for ModerationGuard {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use nest_rs::ws::{Global, WsServer};
    use serde_json::json;

    fn client() -> WsClient {
        WsClient::new(0, Arc::new(WsServer::<Global>::default()))
    }

    #[tokio::test]
    async fn rejects_a_banned_author() {
        let denied = ModerationGuard
            .check_ws_message(
                &client(),
                "message",
                &json!({ "author": "banned", "text": "x" }),
            )
            .await;
        assert!(denied.is_err());
    }

    #[tokio::test]
    async fn allows_everyone_else() {
        let ok = ModerationGuard
            .check_ws_message(
                &client(),
                "message",
                &json!({ "author": "ada", "text": "x" }),
            )
            .await;
        assert!(ok.is_ok());
    }
}
