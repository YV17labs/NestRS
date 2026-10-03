use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use nest_rs::core::injectable;

/// Microseconds since the Unix epoch: the one clock every process of a run
/// reads, so a push and the handler that runs it compare on the same host.
pub fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
        })
}

/// One handler run, as the parent reads it back.
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub seq: u32,
    pub pushed_us: u64,
    pub started_us: u64,
    pub done_us: u64,
}

/// What the handlers of one replica ran since the reporter last asked.
#[injectable]
#[derive(Default)]
pub struct Probe {
    runs: Mutex<Vec<Run>>,
}

impl Probe {
    pub fn record(&self, run: Run) {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(run);
    }

    pub fn take(&self) -> Vec<Run> {
        std::mem::take(&mut *self.runs.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
