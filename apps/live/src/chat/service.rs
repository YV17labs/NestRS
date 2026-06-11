use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::injectable;
use nest_rs_ws::WsServer;
use parking_lot::Mutex;

use crate::chat::dto::{ChatMessage, SendMessage};

#[injectable]
pub struct RoomService {
    #[inject]
    server: Arc<WsServer>,
    history: Mutex<Vec<ChatMessage>>,
    present: AtomicUsize,
}

impl RoomService {
    pub fn connected(&self) {
        self.present.fetch_add(1, Ordering::Relaxed);
    }

    pub fn disconnected(&self) {
        self.present.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn present(&self) -> usize {
        self.present.load(Ordering::Relaxed)
    }

    pub fn record(&self, message: SendMessage) -> ChatMessage {
        let stored = ChatMessage {
            author: message.author,
            text: message.text,
        };
        let mut history = self.history.lock();
        history.push(stored.clone());
        let total = history.len();
        drop(history);

        let reached = self.server.broadcast("message", &stored).unwrap_or(0);
        tracing::info!(
            target: "live::chat",
            author = %stored.author,
            total,
            reached,
            "chat message recorded and broadcast",
        );
        stored
    }

    pub fn history(&self) -> Vec<ChatMessage> {
        self.history.lock().clone()
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
            present: AtomicUsize::new(0),
        };
        let stored = room.record(SendMessage {
            author: "ada".into(),
            text: "hello".into(),
        });
        assert_eq!(stored.text, "hello");
        assert_eq!(room.history().len(), 1);
    }
}
