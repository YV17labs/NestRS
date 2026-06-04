use nestrs_core::injectable;
use nestrs_ws::serde_json::Value;
use nestrs_ws::{MessageGuard, WsClient, async_trait};

#[injectable]
#[derive(Default)]
pub struct ModeratedGuard;

#[async_trait]
impl MessageGuard for ModeratedGuard {
    async fn can_activate(
        &self,
        _client: &WsClient,
        _event: &str,
        data: &Value,
    ) -> Result<(), String> {
        match data.get("author").and_then(Value::as_str) {
            Some("banned") => Err("author `banned` is not allowed to post".into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use nestrs_ws::{Global, WsServer};
    use serde_json::json;

    fn client() -> WsClient {
        WsClient::new(0, Arc::new(WsServer::<Global>::default()))
    }

    #[tokio::test]
    async fn rejects_a_banned_author() {
        let denied = ModeratedGuard
            .can_activate(
                &client(),
                "message",
                &json!({ "author": "banned", "text": "x" }),
            )
            .await;
        assert!(denied.is_err());
    }

    #[tokio::test]
    async fn allows_everyone_else() {
        let ok = ModeratedGuard
            .can_activate(
                &client(),
                "message",
                &json!({ "author": "ada", "text": "x" }),
            )
            .await;
        assert!(ok.is_ok());
    }
}
