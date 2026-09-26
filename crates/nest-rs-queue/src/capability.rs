//! [`Capability`] and [`Capabilities`] — what a queue backend supports beyond
//! the contract every backend owes.
//!
//! The module is private and its types are re-exported flat, so **what a driver
//! author has to read lives on [`Capability`]** rather than here: a `//!` on a
//! private module renders nowhere, and the paragraph below it is the one telling
//! a driver which of three obligations is actually theirs.

use std::fmt;

/// One optional capability of a queue backend: what it supports beyond the
/// contract every backend owes.
///
/// A backend declares the ones it honours in its
/// [`QueueBackend`](crate::QueueBackend), and the port refuses a declaration the
/// backend lacks at the earliest site that sees both facts — the worker's boot
/// for a `#[process]` key, the push for a push option, the call for a cancel.
///
/// **Not a capability, because no backend may refuse them:** the retry budget
/// and its backoff (`#[process(retries = N)]`), one transaction per attempt
/// (`transactional`), and per-method concurrency (`concurrency = N`).
///
/// **Who honours each is a different fact, and only the third answer is the
/// backend.** The budget is the *port's* — [`consume::attempt`](crate::consume)
/// counts it and says how long to wait before the next attempt, so an adapter
/// never counts — and `transactional` is the *worker's*, honoured by whichever
/// `JobContext` the container holds (`nest-rs-seaorm`'s, today), which is why
/// the word appears nowhere in `nest-rs-redis`. A driver author reading one
/// sentence for all three was being told to implement a retry loop that counts
/// and a database transaction per attempt; what a backend actually owes here is
/// `concurrency`, because any backend bounds in-process parallelism with a
/// semaphore.
///
/// Non-exhaustive: the port may name a capability later, and a backend that
/// matches on this enum must not stop compiling the day it does — nor claim the
/// new one, which it cannot have been written to honour.
///
/// "Capability" here means what a driver supports. The umbrella's Cargo features
/// are called capabilities too, and are a different thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capability {
    /// A job held back until a delay passes: `PushOptions::with_delay`.
    DelayedPush,
    /// A second pending job under one key refused: `PushOptions::with_unique`.
    UniquePush,
    /// A pushed job removed before it starts: `JobProducerExt::cancel`, and
    /// `cancel_unique` beside [`UniquePush`](Self::UniquePush).
    Cancellation,
    /// A rate at which a method's attempts start, across the deployment:
    /// `#[process(throttle(..))]`.
    Throttle,
    /// Progress a job keeps across its retries and redeliveries: a
    /// `Checkpoint<_>` parameter.
    Checkpoint,
}

impl Capability {
    /// Every capability, in declaration order — what `Capabilities` derives its
    /// own `ALL` and its iterator from. `pub(crate)`: a driver declares the
    /// capabilities it honours one by one, and never enumerates the port's.
    pub(crate) const ALL: [Self; 5] = [
        Self::DelayedPush,
        Self::UniquePush,
        Self::Cancellation,
        Self::Throttle,
        Self::Checkpoint,
    ];

    const fn bit(self) -> u16 {
        1 << self as u16
    }

    /// What a developer writes that needs this capability — the half of a
    /// refusal that says where to look. `pub(crate)`: it is read by
    /// `unsupported`, which words the whole refusal.
    pub(crate) const fn declared_by(self) -> &'static str {
        match self {
            Self::DelayedPush => "`PushOptions::with_delay`",
            Self::UniquePush => "`PushOptions::with_unique`",
            Self::Cancellation => "`JobProducerExt::cancel` or `cancel_unique`",
            Self::Throttle => "`#[process(throttle(..))]`",
            Self::Checkpoint => "a `Checkpoint<_>` parameter",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DelayedPush => "delayed delivery",
            Self::UniquePush => "unique jobs",
            Self::Cancellation => "job cancellation",
            Self::Throttle => "throttling",
            Self::Checkpoint => "checkpoints",
        })
    }
}

/// A set of [`Capability`] — what one backend declares, or what one declaration
/// needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities(u16);

impl Capabilities {
    /// No optional capability.
    pub const NONE: Self = Self(0);
    /// Every optional capability — the set the bit layout is checked against.
    /// Never public, and outside the tests never needed: a backend declares what
    /// it honours one capability at a time, so a capability the port adds later
    /// is never claimed by a backend written before it existed.
    #[cfg(test)]
    pub(crate) const ALL: Self = Self((1 << Capability::ALL.len()) - 1);

    /// This set with `capability` added.
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.bit())
    }

    /// Whether `capability` is in the set.
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    /// The capabilities in the set, in declaration order.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |capability| self.contains(*capability))
    }
}

/// The refusal every site prints for a declaration its backend cannot honour,
/// worded once so a boot error and a push error read alike. `site` names the
/// `#[process]` method the key sits on when the refusal is the boot's.
pub(crate) fn unsupported(capability: Capability, backend: &str, site: Option<&str>) -> String {
    let on = site.map(|site| format!(" on `{site}`")).unwrap_or_default();
    format!(
        "{}{on} needs {capability}, which the `{backend}` queue backend does not provide",
        capability.declared_by(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_a_bit_of_its_own() {
        let mut seen = Capabilities::NONE;
        for capability in Capability::ALL {
            assert!(!seen.contains(capability), "{capability:?} shares a bit");
            seen = seen.with(capability);
        }
        assert_eq!(seen, Capabilities::ALL);
        assert_eq!(Capabilities::ALL.iter().count(), Capability::ALL.len());
        assert_eq!(Capabilities::NONE.iter().count(), 0);
    }

    #[test]
    fn the_refusal_names_the_declaration_the_site_and_the_backend() {
        let sentence = unsupported(
            Capability::Throttle,
            "nats",
            Some("BillingProcessor::charge"),
        );
        assert_eq!(
            sentence,
            "`#[process(throttle(..))]` on `BillingProcessor::charge` needs throttling, which \
             the `nats` queue backend does not provide",
        );
    }
}
