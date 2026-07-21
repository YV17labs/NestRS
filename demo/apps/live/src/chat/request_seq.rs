//! A request-scoped provider reached from a WS message handler — the demo proof
//! that `#[injectable(scope = request)]` providers are reachable over WS through
//! `nest_rs_ws::Scoped<T>`, at per-message scope (the four-transport parity item).

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs_core::injectable;

/// Singleton counter shared across the whole app — the source a per-message
/// [`RequestSeq`] stamps itself from.
#[injectable]
#[derive(Default)]
pub struct SeqSource {
    next: AtomicU64,
}

impl SeqSource {
    /// The next monotonic sequence number.
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}

/// A per-message provider: built fresh for each inbound WS message, it stamps a
/// sequence number from the singleton [`SeqSource`] the first time it is read.
/// Because a new instance (and a fresh `OnceLock`) exists per message, two
/// messages observe two distinct values — a singleton would keep the first
/// forever. Reached only via [`nest_rs_ws::Scoped`], never `#[inject]`.
#[injectable(scope = request)]
pub struct RequestSeq {
    #[inject]
    source: Arc<SeqSource>,
    // Non-`#[inject]` field: `Default`-constructed (an empty `OnceLock`) per
    // message, stamped on first read so the value is stable within the message.
    seq: OnceLock<u64>,
}

impl RequestSeq {
    /// This message's sequence number — stamped once, on first read.
    pub fn value(&self) -> u64 {
        *self.seq.get_or_init(|| self.source.next())
    }
}
