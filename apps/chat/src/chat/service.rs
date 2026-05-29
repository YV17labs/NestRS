use std::sync::Mutex;

use nestrs_core::injectable;

use crate::chat::dto::{ChatMessage, SendMessage};

#[injectable]
#[derive(Default)]
pub struct RoomService {
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
        tracing::info!(
            target: "chat",
            author = %stored.author,
            total = history.len(),
            "chat message recorded",
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
        let room = RoomService::default();
        let stored = room.record(SendMessage {
            author: "ada".into(),
            text: "hello".into(),
        });
        assert_eq!(stored.text, "hello");
        assert_eq!(room.history().len(), 1);
    }
}
