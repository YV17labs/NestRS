//! [`ProcessOptions`] and [`Throttle`] — what a `#[process]` method declares
//! about how its jobs run.

use std::num::NonZeroU32;
use std::time::Duration;

/// A rate at which a method's attempts may start: at most `limit` starts per
/// `window`, **across the deployment**.
///
/// Every replica draining the queue counts against the one limit, and a retry
/// is an attempt like any other. How the backend cuts time into windows — fixed
/// or sliding — and how far replicas racing at a window's edge can exceed the
/// limit is the backend's to state on its own type; a backend that cannot keep
/// the rate across replicas at all does not declare
/// [`Capability::Throttle`](crate::Capability::Throttle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Throttle {
    limit: NonZeroU32,
    window: Duration,
}

impl Throttle {
    /// The shortest window a throttle counts over: a millisecond, the resolution
    /// of the coarsest store the framework ships — Redis's `PEXPIRE` — and the
    /// same floor as `nest_rs_throttler::Throttle::MIN_WINDOW`, the other rate
    /// the framework declares. A shorter window limits nothing, and the boot
    /// refuses one ([`QueueWorker`](crate::QueueWorker)).
    pub const MIN_WINDOW: Duration = Duration::from_millis(1);

    /// `limit` attempt starts per `window`.
    pub const fn new(limit: NonZeroU32, window: Duration) -> Self {
        Self { limit, window }
    }

    /// How many attempts may start in one window.
    pub const fn limit(&self) -> NonZeroU32 {
        self.limit
    }

    /// The window the limit applies over.
    pub const fn window(&self) -> Duration {
        self.window
    }
}

/// Everything a `#[process]` method declares besides its queue — built by the
/// decorator, read by a backend.
///
/// Every setter is `const`, so the decorator writes one expression the
/// link-time inventory can hold, and a key added later is one more setter
/// rather than one more field in every literal that builds the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProcessOptions {
    retries: u32,
    concurrency: NonZeroU32,
    throttle: Option<Throttle>,
    checkpoint: bool,
    timeout: Duration,
}

impl ProcessOptions {
    /// One job at a time, no retry, no throttle, no checkpoint, and the
    /// family's timeout ([`JOB_TIMEOUT`](nest_rs_worker::JOB_TIMEOUT)).
    pub const DEFAULT: Self = Self {
        retries: 0,
        concurrency: NonZeroU32::MIN,
        throttle: None,
        checkpoint: false,
        timeout: nest_rs_worker::JOB_TIMEOUT,
    };

    /// `retries = N`: re-run a job whose attempt failed retryably, up to `N`
    /// times after the first attempt, each after the port's backoff.
    pub const fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// `concurrency = N`: at most `N` attempts of this method at once in one
    /// worker replica. Not a capability: every backend honours it.
    pub const fn with_concurrency(mut self, concurrency: NonZeroU32) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// `throttle(limit = N, window = "…")`: at most `N` attempt starts per
    /// window, across the deployment.
    pub const fn with_throttle(mut self, throttle: Throttle) -> Self {
        self.throttle = Some(throttle);
        self
    }

    /// The method takes a `Checkpoint<_>` parameter.
    pub const fn with_checkpoint(mut self, checkpoint: bool) -> Self {
        self.checkpoint = checkpoint;
        self
    }

    /// `timeout = "…"`: how long an attempt runs before it is cut and fails,
    /// retryably.
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Re-runs after a first failed attempt.
    pub const fn retries(&self) -> u32 {
        self.retries
    }

    /// Attempts of this method at once, per worker replica.
    pub const fn concurrency(&self) -> NonZeroU32 {
        self.concurrency
    }

    /// The rate attempts of this method may start at, if limited.
    pub const fn throttle(&self) -> Option<Throttle> {
        self.throttle
    }

    /// Whether the method takes a `Checkpoint<_>` parameter.
    pub const fn checkpoint(&self) -> bool {
        self.checkpoint
    }

    /// How long an attempt runs before it is cut.
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}
