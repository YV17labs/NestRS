use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::injectable;
use nest_rs_ws::{WsServer, serde_json};
use parking_lot::Mutex;

use crate::chat::dtos::{ChatMessageDto, SendMessageDto};

/// Cap on the in-memory chat scrollback. The buffer is a singleton every
/// connected client can append to, so it must be bounded: at capacity the
/// oldest message is dropped (ring buffer) to keep process memory flat.
const HISTORY_CAPACITY: usize = 256;

#[injectable]
pub struct ChatService {
    #[inject]
    server: Arc<WsServer>,
    history: Mutex<VecDeque<ChatMessageDto>>,
    present: AtomicUsize,
}

impl ChatService {
    pub fn connected(&self) {
        self.present.fetch_add(1, Ordering::Relaxed);
    }

    pub fn disconnected(&self) {
        self.present.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn present(&self) -> usize {
        self.present.load(Ordering::Relaxed)
    }

    /// Records the message to the scrollback and broadcasts it live. A broadcast
    /// failure is **propagated**, not swallowed: `#[messages]` turns the `Err`
    /// into the dispatch-layer `warn` plus an error frame to the sender, so a
    /// message the room never received is never reported as delivered.
    pub fn record(&self, message: SendMessageDto) -> Result<ChatMessageDto, serde_json::Error> {
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

        let reached = self.server.broadcast("message", &stored)?;
        tracing::debug!(
            target: "live::chat",
            author = %stored.author,
            total,
            reached,
            "chat message recorded and broadcast",
        );
        Ok(stored)
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
        let svc = ChatService {
            server: Arc::new(WsServer::default()),
            history: Mutex::new(VecDeque::new()),
            present: AtomicUsize::new(0),
        };
        let stored = svc
            .record(SendMessageDto {
                author: "ada".into(),
                text: "hello".into(),
            })
            .expect("broadcast serializes a two-field DTO");
        assert_eq!(stored.text, "hello");
        assert_eq!(svc.history().len(), 1);
    }
}
