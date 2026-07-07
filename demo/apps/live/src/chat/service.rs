use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::injectable;
use nest_rs_ws::WsServer;
use parking_lot::Mutex;

use crate::chat::dtos::{ChatMessageDto, SendMessageDto};

/// Cap on the in-memory chat scrollback. The buffer is a singleton every
/// connected client can append to, so it must be bounded: at capacity the
/// oldest message is dropped (ring buffer) to keep process memory flat.
const HISTORY_CAPACITY: usize = 256;

#[injectable]
pub struct RoomService {
    #[inject]
    server: Arc<WsServer>,
    history: Mutex<VecDeque<ChatMessageDto>>,
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

    pub fn record(&self, message: SendMessageDto) -> ChatMessageDto {
        let stored = ChatMessageDto {
            author: message.author,
            text: message.text,
        };
        let mut history = self.history.lock();
        if history.len() >= HISTORY_CAPACITY {
            history.pop_front();
        }
        history.push_back(stored.clone());
        let total = history.len();
        drop(history);

        match self.server.broadcast("message", &stored) {
            Ok(reached) => tracing::info!(
                target: "live::chat",
                author = %stored.author,
                total,
                reached,
                "chat message recorded and broadcast",
            ),
            Err(e) => tracing::warn!(
                target: "live::chat",
                error = %e,
                "broadcast failed",
            ),
        }
        stored
    }

    pub fn history(&self) -> Vec<ChatMessageDto> {
        self.history.lock().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_returns_history() {
        let room = RoomService {
            server: Arc::new(WsServer::default()),
            history: Mutex::new(VecDeque::new()),
            present: AtomicUsize::new(0),
        };
        let stored = room.record(SendMessageDto {
            author: "ada".into(),
            text: "hello".into(),
        });
        assert_eq!(stored.text, "hello");
        assert_eq!(room.history().len(), 1);
    }
}
