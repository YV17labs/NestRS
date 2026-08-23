use std::sync::Arc;
use std::sync::OnceLock;

use nest_rs::core::injectable;

use super::seq_source::SeqSource;

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
