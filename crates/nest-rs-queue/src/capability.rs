//! [`Capability`] and [`Capabilities`] — what a queue backend supports beyond
//! the contract every backend owes.

use std::fmt;

/// One optional capability of a queue backend: what it supports beyond the
/// contract every backend owes.
///
/// A backend declares the ones it honours in its
/// [`QueueBackend`](crate::QueueBackend), and the port refuses a declaration the
/// backend lacks at the earliest site that sees both facts — the worker's boot
/// for a `#[process]` key, the push for a push option, the call for a cancel.
///
/// **Not a capability, and none the backend's to keep:** the retry budget and
/// its backoff (`#[process(retries = N)]`) and per-method concurrency
/// (`concurrency = N`) are the port's; one transaction per attempt
/// (`transactional`) is the `JobContext`'s the container holds.
///
/// Not the umbrella's Cargo features, which are called capabilities too.
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
    /// Every capability, in declaration order; never public, since a driver
    /// declares the capabilities it honours one by one.
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
    /// refusal that says where to look.
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
    use std::any::TypeId;
    use std::num::NonZeroU32;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::consume::unsupported_by;
    use crate::inventory::{HandlerContext, JobHandler};
    use crate::{
        Envelope, JobError, JobId, JobProducer, JobProducerExt, ProcessMethod, ProcessOptions,
        PushOptions, PushReceipt, QueueBackend, QueueError, QueueName, Throttle,
    };

    /// A backend declaring no optional capability, so every declaration that
    /// needs one is refused before it is reached.
    static NOTHING: QueueBackend = QueueBackend::new("nothing", Capabilities::NONE);

    const QUEUE: &str = "audio";
    const METHOD: &str = "AudioProcessor::transcode";

    struct Producer;

    #[async_trait]
    impl JobProducer for Producer {
        fn backend(&self) -> &'static QueueBackend {
            &NOTHING
        }

        async fn enqueue(
            &self,
            _: &QueueName,
            _: Vec<Envelope>,
            _: &PushOptions,
        ) -> Result<(), QueueError> {
            unreachable!("a refused push never reaches its backend")
        }
    }

    fn handler(
        _: std::borrow::Cow<'_, Value>,
        _: HandlerContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), JobError>> + Send + '_>>
    {
        Box::pin(async { Ok(()) })
    }

    /// The boot's refusals of a `#[process]` method declaring `options`.
    fn boot(options: ProcessOptions) -> String {
        let method = ProcessMethod::new(
            module_path!(),
            METHOD,
            QUEUE,
            options,
            TypeId::of::<Producer>,
            handler as JobHandler,
        );
        unsupported_by(&method, &NOTHING)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn refused<T: std::fmt::Debug>(answer: Result<T, QueueError>) -> String {
        answer.expect_err("refused").to_string()
    }

    /// What a developer writes that needs `capability`, at the site that meets
    /// it, against [`NOTHING`]. Exhaustive, so a capability added to the port
    /// does not compile until it names the declaration that needs it.
    async fn refusal(capability: Capability) -> String {
        let push = |options| Producer.push_json(QUEUE, Value::Null, options);
        match capability {
            Capability::DelayedPush => {
                refused(push(PushOptions::default().with_delay(Duration::from_secs(1))).await)
            }
            Capability::UniquePush => {
                refused(push(PushOptions::default().with_unique("clip")).await)
            }
            Capability::Cancellation => {
                let queue = QueueName::new(QUEUE).expect("a valid name");
                refused(
                    Producer
                        .cancel(&PushReceipt::new(queue, JobId::mint()))
                        .await,
                )
            }
            Capability::Throttle => boot(
                ProcessOptions::DEFAULT
                    .with_throttle(Throttle::new(NonZeroU32::MIN, Duration::from_secs(1))),
            ),
            Capability::Checkpoint => boot(ProcessOptions::DEFAULT.with_checkpoint(true)),
        }
    }

    #[tokio::test]
    async fn every_capability_is_refused_where_it_is_declared_on_a_backend_without_it() {
        for capability in Capability::ALL {
            let sentence = refusal(capability).await;
            assert!(
                [None, Some(METHOD)]
                    .map(|site| unsupported(capability, NOTHING.name(), site))
                    .contains(&sentence),
                "{capability:?} refused as {sentence:?}",
            );
        }
    }

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
