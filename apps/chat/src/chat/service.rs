use std::sync::{Arc, Mutex};

use nestrs_core::injectable;
use nestrs_ws::WsServer;

use crate::chat::dto::{ChatMessage, SendMessage};

#[injectable]
pub struct RoomService {
    #[inject]
    server: Arc<WsServer>,
    history: Mutex<Vec<ChatMessage>>,
}

impl RoomService {
    pub fn record(&self, message: SendMessage) -> ChatMessage {
        let stored = ChatMessage {
            author: message.author,
            text: message.text,
        };
        let mut history = self.history.lock().unwrap();
        history.push(stored.clone());
        let total = history.len();
        drop(history);

        let reached = self.server.broadcast("message", &stored).unwrap_or(0);
        tracing::info!(
            target: "chat",
            author = %stored.author,
            total,
            reached,
            "chat message recorded and broadcast",
        );
        stored
    }

    pub fn history(&self) -> Vec<ChatMessage> {
        self.history.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_returns_history() {
        let room = RoomService {
            server: Arc::new(WsServer::default()),
            history: Mutex::new(Vec::new()),
        };
        let stored = room.record(SendMessage {
            author: "ada".into(),
            text: "hello".into(),
        });
        assert_eq!(stored.text, "hello");
        assert_eq!(room.history().len(), 1);
    }
}
