use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
pub struct SeqSource {
    next: AtomicU64,
}

impl SeqSource {
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}
