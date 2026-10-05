//! [`KitCommand`] — the job every case pushes — and one queue per case, so the
//! kit's cases run side by side on one backend without taking each other's
//! jobs.

use nest_rs_queue::queue;
use serde::{Deserialize, Serialize};

/// A case's job: which run of the case pushed it, its number in the run, and
/// what its attempts do.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct KitCommand {
    pub(crate) run: u64,
    pub(crate) seq: u32,
    pub(crate) act: Act,
}

/// What an attempt at a [`KitCommand`] does.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum Act {
    /// Complete at once.
    Complete,
    /// Complete after `ms` milliseconds.
    Hold { ms: u64 },
    /// Fail, retryably, on the first `attempts` attempts; complete after.
    FailFirst { attempts: u32 },
    /// Fail, retryably, every attempt.
    FailAlways,
    /// Wait for ever on the first attempt; complete on every later one.
    ParkOnce,
    /// Wait for ever, every attempt.
    ParkAlways,
}

#[queue(name = "nestrs-kit-once", job = KitCommand)]
pub(crate) struct OnceQueue;

#[queue(name = "nestrs-kit-concurrency", job = KitCommand)]
pub(crate) struct ConcurrencyQueue;

#[queue(name = "nestrs-kit-retry", job = KitCommand)]
pub(crate) struct RetryQueue;

#[queue(name = "nestrs-kit-budget", job = KitCommand)]
pub(crate) struct BudgetQueue;

#[queue(name = "nestrs-kit-death", job = KitCommand)]
pub(crate) struct DeathQueue;

#[queue(name = "nestrs-kit-taken", job = KitCommand)]
pub(crate) struct TakenQueue;

#[queue(name = "nestrs-kit-stall", job = KitCommand)]
pub(crate) struct StallQueue;

#[queue(name = "nestrs-kit-drain", job = KitCommand)]
pub(crate) struct DrainQueue;

#[queue(name = "nestrs-kit-renewal", job = KitCommand)]
pub(crate) struct RenewalQueue;

#[queue(name = "nestrs-kit-delay", job = KitCommand)]
pub(crate) struct DelayQueue;

#[queue(name = "nestrs-kit-trace", job = KitCommand)]
pub(crate) struct TraceQueue;
