//! What the kit's processor reports, and what a case reads: every attempt's
//! start and end, per queue, in the process the case runs in.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::command::KitCommand;

static PROBE: Mutex<Option<Probe>> = Mutex::new(None);

#[derive(Default)]
struct Probe {
    attempts: Vec<Attempt>,
    running: HashMap<&'static str, u32>,
    peak: HashMap<&'static str, u32>,
}

/// One attempt the processor ran.
#[derive(Clone, Debug)]
pub(crate) struct Attempt {
    pub(crate) queue: &'static str,
    pub(crate) run: u64,
    pub(crate) seq: u32,
    pub(crate) started: Instant,
    /// `Some(true)` completed, `Some(false)` failed, `None` still running or cut.
    pub(crate) ended: Option<bool>,
    /// The trace the attempt ran in.
    pub(crate) trace: Option<String>,
}

fn probe() -> MutexGuard<'static, Option<Probe>> {
    PROBE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An attempt's place in the probe: reported running until it is dropped.
pub(crate) struct Running {
    index: usize,
    queue: &'static str,
}

/// Record the start of an attempt at `job` on `queue`, returning how many
/// attempts at it started, this one included.
pub(crate) fn start(queue: &'static str, job: &KitCommand) -> (Running, u32) {
    let mut guard = probe();
    let probe = guard.get_or_insert_with(Probe::default);
    probe.attempts.push(Attempt {
        queue,
        run: job.run,
        seq: job.seq,
        started: Instant::now(),
        ended: None,
        trace: nest_rs_core::current_trace_id().map(|trace| trace.to_string()),
    });
    let running = probe.running.entry(queue).or_default();
    *running += 1;
    let now = *running;
    let peak = probe.peak.entry(queue).or_default();
    *peak = (*peak).max(now);
    let attempt = probe
        .attempts
        .iter()
        .filter(|attempt| {
            attempt.queue == queue && attempt.run == job.run && attempt.seq == job.seq
        })
        .count();
    (
        Running {
            index: probe.attempts.len() - 1,
            queue,
        },
        u32::try_from(attempt).unwrap_or(u32::MAX),
    )
}

impl Running {
    /// The attempt ended, `ok` or failed.
    pub(crate) fn end(self, ok: bool) {
        if let Some(probe) = probe().as_mut()
            && let Some(attempt) = probe.attempts.get_mut(self.index)
        {
            attempt.ended = Some(ok);
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(probe) = probe().as_mut()
            && let Some(running) = probe.running.get_mut(self.queue)
        {
            *running = running.saturating_sub(1);
        }
    }
}

/// Every attempt at job `seq` of `run` on `queue`, in start order.
pub(crate) fn attempts(queue: &'static str, run: u64, seq: u32) -> Vec<Attempt> {
    probe()
        .as_ref()
        .map(|probe| {
            probe
                .attempts
                .iter()
                .filter(|attempt| {
                    attempt.queue == queue && attempt.run == run && attempt.seq == seq
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The most attempts `queue` ran at once.
pub(crate) fn peak(queue: &'static str) -> u32 {
    probe()
        .as_ref()
        .and_then(|probe| probe.peak.get(queue).copied())
        .unwrap_or_default()
}

/// Wait until `done` holds, polling every 10 ms, for at most `within`.
pub(crate) async fn until(within: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what}: not seen within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
