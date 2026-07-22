use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs_core::injectable;

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

#[injectable(scope = request)]
pub struct RequestSeq {
    #[inject]
    source: Arc<SeqSource>,
    seq: OnceLock<u64>,
}

impl RequestSeq {
    pub fn value(&self) -> u64 {
        *self.seq.get_or_init(|| self.source.next())
    }
}
