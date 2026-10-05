//! [`PushOptions`] and [`Delay`] — everything a push can declare besides its
//! queue and its payload.

use std::time::{Duration, SystemTime};

use crate::{Capabilities, Capability, QueueError};

/// When a pushed job becomes available to a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delay {
    /// This long after the push.
    For(Duration),
    /// At this instant. An instant already past is an immediate push.
    Until(SystemTime),
}

impl Delay {
    /// The latest instant a job may be due at, as the time since the Unix
    /// epoch: `9999-12-31T23:59:59.999Z`, the last instant RFC 3339 writes.
    ///
    /// A bound on the *instant*, not on the delay, because the instant is what
    /// a backend stores: every store that keeps a timestamp represents one this
    /// late — Redis's milliseconds and a Lua number among them — which a delay
    /// alone within that range would not hold to.
    pub const LATEST_DUE: Duration = Duration::from_millis(253_402_300_799_999);

    /// The instant the delay ends for a push made at `pushed_at`, refused with
    /// [`QueueError::InvalidOptions`] when it is later than
    /// [`LATEST_DUE`](Self::LATEST_DUE).
    pub fn deadline(self, pushed_at: SystemTime) -> Result<SystemTime, QueueError> {
        let at = match self {
            Self::For(delay) => pushed_at.checked_add(delay),
            Self::Until(at) => Some(at),
        };
        // An instant before the epoch is long past, so an immediate push.
        at.filter(|at| {
            at.duration_since(SystemTime::UNIX_EPOCH)
                .map_or(true, |since| since <= Self::LATEST_DUE)
        })
        .ok_or(QueueError::InvalidOptions {
            reason: "the delay ends after 9999-12-31T23:59:59.999Z, the last instant RFC 3339 \
                     writes and the latest a job may be due at",
        })
    }
}

impl From<Duration> for Delay {
    fn from(delay: Duration) -> Self {
        Self::For(delay)
    }
}

impl From<SystemTime> for Delay {
    fn from(at: SystemTime) -> Self {
        Self::Until(at)
    }
}

/// What a push declares besides its queue and payload.
///
/// `Default` is an immediate, ordinary push; every option is one `with_*` call
/// on the value — `PushOptions::default().with_delay(Duration::from_secs(60))`.
/// A backend declares the options it honours as capabilities, and the push
/// refuses an option the backend lacks before anything reaches it, so no backend
/// ever receives an option it would drop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PushOptions {
    delay: Option<Delay>,
    unique: Option<String>,
}

impl PushOptions {
    /// The longest a unique key may be, in bytes.
    pub const MAX_UNIQUE_KEY_LEN: usize = 256;

    /// Hold the job back: for a [`Duration`] after the push, or until a
    /// [`SystemTime`]. Setting it again replaces it.
    ///
    /// Cluster-wide: the backend holds the job, and any worker replica takes it
    /// once the delay ends. Needs [`Capability::DelayedPush`].
    pub fn with_delay(mut self, delay: impl Into<Delay>) -> Self {
        self.delay = Some(delay.into());
        self
    }

    /// At most one job per queue and `key` pending or running: a push under a
    /// key another job on the same queue still holds is refused with
    /// [`QueueError::UniqueKeyHeld`], which names that job's
    /// [`JobId`](crate::JobId), and files nothing.
    ///
    /// The key is held from the push until its job reaches a terminal outcome —
    /// it completes, it dead-letters, or it is cancelled — and is free for the
    /// next push from then on. [`cancel_unique`](crate::JobProducerExt::cancel_unique)
    /// cancels the job waiting under a key.
    ///
    /// **At-most-once over pushes, never a distributed lock.** It keeps a
    /// sequence of pushes from enqueueing one piece of work twice; it does not
    /// make two replicas, or two methods, take turns at anything, and a job
    /// redelivered after its replica died runs again under the key it already
    /// holds.
    ///
    /// Deployment-wide, and scoped to the queue: the same key on two queues
    /// names two jobs. 1 to [`MAX_UNIQUE_KEY_LEN`](Self::MAX_UNIQUE_KEY_LEN)
    /// bytes, no control character. Needs [`Capability::UniquePush`].
    pub fn with_unique(mut self, key: impl Into<String>) -> Self {
        self.unique = Some(key.into());
        self
    }

    /// The delay declared, if any.
    pub fn delay(&self) -> Option<Delay> {
        self.delay
    }

    /// The unique key declared, if any.
    pub fn unique_key(&self) -> Option<&str> {
        self.unique.as_deref()
    }

    /// The optional capabilities these options need from a backend.
    ///
    /// `#[doc(hidden)]`: the push refuses an option the backend lacks before
    /// anything reaches the backend, so a driver never asks. Public because
    /// `push_values` and the port's own suite read it.
    #[doc(hidden)]
    pub fn required_capabilities(&self) -> Capabilities {
        let mut required = Capabilities::NONE;
        if self.delay.is_some() {
            required = required.with(Capability::DelayedPush);
        }
        if self.unique.is_some() {
            required = required.with(Capability::UniquePush);
        }
        required
    }

    /// Refuse options no backend could honour, before any backend sees them.
    pub(crate) fn check(&self) -> Result<(), QueueError> {
        if let Some(delay) = self.delay {
            delay.deadline(SystemTime::now())?;
        }
        match &self.unique {
            Some(key) => check_unique_key(key),
            None => Ok(()),
        }
    }
}

/// Refuse a unique key no backend could enqueue — at the push declaring it, and at
/// the cancel naming it.
pub(crate) fn check_unique_key(key: &str) -> Result<(), QueueError> {
    let reason = if key.is_empty() {
        "it is empty"
    } else if key.len() > PushOptions::MAX_UNIQUE_KEY_LEN {
        "it is too long"
    } else if key.chars().any(char::is_control) {
        "it holds a control character"
    } else {
        return Ok(());
    };
    Err(QueueError::InvalidUniqueKey { reason })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_is_a_duration_or_an_instant_and_setting_it_again_replaces_it() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let options = PushOptions::default()
            .with_delay(Duration::from_secs(5))
            .with_delay(at);
        assert_eq!(options.delay(), Some(Delay::Until(at)));
        assert_eq!(
            Delay::For(Duration::from_secs(5)).deadline(at).ok(),
            Some(at + Duration::from_secs(5)),
        );
    }

    /// A job is due no later than RFC 3339's last instant, however the delay is
    /// spelled — and refused at the port, before any backend sees it. A delay of
    /// 400 million years reached Redis and came back as a backend failure.
    #[test]
    fn a_delay_ending_after_the_latest_due_instant_is_refused_at_the_port() {
        let latest = SystemTime::UNIX_EPOCH + Delay::LATEST_DUE;
        let now = SystemTime::now();
        assert_eq!(Delay::Until(latest).deadline(now).ok(), Some(latest));
        let refused = [
            Delay::Until(latest + Duration::from_millis(1)),
            Delay::For(Duration::from_secs(400_000_000 * 365 * 86_400)),
            Delay::For(Duration::MAX),
        ];
        for delay in refused {
            assert!(
                matches!(delay.deadline(now), Err(QueueError::InvalidOptions { .. })),
                "{delay:?}"
            );
            let options = PushOptions::default().with_delay(delay);
            let said = options
                .check()
                .expect_err("refused at the port")
                .to_string();
            assert!(said.contains("9999-12-31T23:59:59.999Z"), "{said}");
        }
        assert!(
            PushOptions::default()
                .with_delay(Duration::from_secs(1000 * 365 * 86_400))
                .check()
                .is_ok(),
            "a thousand years is still an instant RFC 3339 writes"
        );
    }

    #[test]
    fn each_option_needs_its_capability_and_default_needs_none() {
        assert_eq!(
            PushOptions::default().required_capabilities(),
            Capabilities::NONE
        );
        let both = PushOptions::default()
            .with_delay(Duration::from_secs(1))
            .with_unique("post-42")
            .required_capabilities();
        assert!(both.contains(Capability::DelayedPush));
        assert!(both.contains(Capability::UniquePush));
    }

    #[test]
    fn a_unique_key_no_backend_could_file_is_refused() {
        for refused in [
            String::new(),
            "x".repeat(PushOptions::MAX_UNIQUE_KEY_LEN + 1),
            "post\n42".to_owned(),
        ] {
            let options = PushOptions::default().with_unique(refused.clone());
            assert!(
                matches!(options.check(), Err(QueueError::InvalidUniqueKey { .. })),
                "{refused:?}",
            );
        }
        assert!(
            PushOptions::default()
                .with_unique("post-42 · résumé")
                .check()
                .is_ok()
        );
    }

    /// The refusal states the rule with the limit the check reads, so the
    /// sentence and the check cannot disagree about it.
    #[test]
    fn a_refused_key_is_told_the_limit_the_check_reads() {
        let refused = PushOptions::default()
            .with_unique("x".repeat(PushOptions::MAX_UNIQUE_KEY_LEN + 1))
            .check()
            .expect_err("too long")
            .to_string();
        assert!(
            refused.contains("too long")
                && refused.contains(&format!("1 to {} bytes", PushOptions::MAX_UNIQUE_KEY_LEN)),
            "{refused}"
        );
    }
}
