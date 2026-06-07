use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_ws::serde_json::Value;
use nest_rs_ws::{WsClient, async_trait};

#[injectable]
#[derive(Default)]
pub struct ModeratedGuard;

impl Layer for ModeratedGuard {}

#[async_trait]
impl Guard for ModeratedGuard {
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        data: &Value,
    ) -> Result<(), Denial> {
        match data.get("author").and_then(Value::as_str) {
            Some("banned") => Err(Denial::forbidden("author `banned` is not allowed to post")),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use nest_rs_ws::{Global, WsServer};
    use serde_json::json;

    fn client() -> WsClient {
        WsClient::new(0, Arc::new(WsServer::<Global>::default()))
    }

    #[tokio::test]
    async fn rejects_a_banned_author() {
        let denied = ModeratedGuard
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
        let ok = ModeratedGuard
            .check_ws_message(
                &client(),
                "message",
                &json!({ "author": "ada", "text": "x" }),
            )
            .await;
        assert!(ok.is_ok());
    }
}
