//! The counters a policy records on — its breaker's transitions, its retries,
//! the refusals its runs end on and the fallbacks answering in place of a
//! run's error — each made at its first record, on the global meter installed
//! then: first recorded after `OpenTelemetry::init()` it counts, its policy
//! built before it or after, and first recorded before it, it stays on the
//! meter that records nothing. What a policy holds now is observed beside
//! them ([`Observed`](crate::observed::Observed)). Without the
//! `opentelemetry` feature a record is nothing.

use crate::error::Refusal;

/// Each transition of a breaker, by the state it entered.
#[cfg(feature = "opentelemetry")]
pub(crate) const TRANSITIONS: &str = "nestrs.resilience.breaker.transitions";

/// Each call a policy sent again.
#[cfg(feature = "opentelemetry")]
pub(crate) const RETRIES: &str = "nestrs.resilience.retries";

/// Each run over at its policy's own refusal, by the refusal: the one the run
/// answers, but for the timeout of a last attempt whose savepoint cannot be
/// rolled back, counted as that attempt ends, its run then answering
/// `Unsettled`.
#[cfg(feature = "opentelemetry")]
pub(crate) const REFUSALS: &str = "nestrs.resilience.refusals";

/// Each run whose fallback answered in place of its error.
#[cfg(feature = "opentelemetry")]
pub(crate) const FALLBACKS: &str = "nestrs.resilience.fallbacks";

#[cfg(feature = "opentelemetry")]
pub(crate) const INSTANCE_ATTRIBUTE: &str = "nestrs.resilience.instance";

#[cfg(feature = "opentelemetry")]
pub(crate) const STATE_ATTRIBUTE: &str = "nestrs.resilience.breaker.state";

/// `true` on a manual transition, and absent from every other.
#[cfg(feature = "opentelemetry")]
pub(crate) const MANUAL_ATTRIBUTE: &str = "nestrs.resilience.breaker.manual";

#[cfg(feature = "opentelemetry")]
pub(crate) const REFUSAL_ATTRIBUTE: &str = "nestrs.resilience.refusal";

/// One policy's counters, by its instance.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    #[cfg(feature = "opentelemetry")]
    instance: &'static str,
    #[cfg(feature = "opentelemetry")]
    transitions: Lazy,
    #[cfg(feature = "opentelemetry")]
    retries: Lazy,
    #[cfg(feature = "opentelemetry")]
    refusals: Lazy,
    #[cfg(feature = "opentelemetry")]
    fallbacks: Lazy,
}

/// A counter made at its first record, and kept.
#[cfg(feature = "opentelemetry")]
type Lazy = std::sync::OnceLock<opentelemetry::metrics::Counter<u64>>;

impl Counters {
    pub(crate) fn new(instance: &'static str) -> Self {
        #[cfg(not(feature = "opentelemetry"))]
        let _ = instance;
        Self {
            #[cfg(feature = "opentelemetry")]
            instance,
            ..Self::default()
        }
    }

    /// A transition into `state`, `manual` or not.
    pub(crate) fn transition(&self, state: &'static str, manual: bool) {
        #[cfg(feature = "opentelemetry")]
        {
            use opentelemetry::KeyValue;

            let state = KeyValue::new(STATE_ATTRIBUTE, state);
            let counter = made(
                &self.transitions,
                TRANSITIONS,
                "{transition}",
                "The circuit breakers' transitions, by the state each entered, and whether it \
                 was manual.",
            );
            if manual {
                counter.add(
                    1,
                    &[
                        self.instance(),
                        state,
                        KeyValue::new(MANUAL_ATTRIBUTE, true),
                    ],
                );
            } else {
                counter.add(1, &[self.instance(), state]);
            }
        }
        #[cfg(not(feature = "opentelemetry"))]
        let _ = (state, manual);
    }

    pub(crate) fn retry(&self) {
        #[cfg(feature = "opentelemetry")]
        made(
            &self.retries,
            RETRIES,
            "{retry}",
            "The calls the policies sent again.",
        )
        .add(1, &[self.instance()]);
    }

    pub(crate) fn refusal(&self, refusal: Refusal) {
        #[cfg(feature = "opentelemetry")]
        made(
            &self.refusals,
            REFUSALS,
            "{refusal}",
            "The runs the policies ended on their own refusal, by the refusal.",
        )
        .add(
            1,
            &[
                self.instance(),
                opentelemetry::KeyValue::new(REFUSAL_ATTRIBUTE, refusal.as_str()),
            ],
        );
        #[cfg(not(feature = "opentelemetry"))]
        let _ = refusal;
    }

    pub(crate) fn fallback(&self) {
        #[cfg(feature = "opentelemetry")]
        made(
            &self.fallbacks,
            FALLBACKS,
            "{fallback}",
            "The runs a fallback answered in place of their error.",
        )
        .add(1, &[self.instance()]);
    }

    #[cfg(feature = "opentelemetry")]
    fn instance(&self) -> opentelemetry::KeyValue {
        opentelemetry::KeyValue::new(INSTANCE_ATTRIBUTE, self.instance)
    }
}

/// The counter `lazy` holds, made at its first record on the global meter
/// installed then: one made before `OpenTelemetry::init()` records nothing,
/// after it too.
#[cfg(feature = "opentelemetry")]
fn made<'c>(
    lazy: &'c Lazy,
    name: &'static str,
    unit: &'static str,
    description: &'static str,
) -> &'c opentelemetry::metrics::Counter<u64> {
    lazy.get_or_init(|| {
        opentelemetry::global::meter("nestrs")
            .u64_counter(name)
            .with_unit(unit)
            .with_description(description)
            .build()
    })
}

#[cfg(test)]
#[cfg(feature = "opentelemetry")]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;
    use crate::breaker::{Breaker, BreakerSettings};
    use crate::teller::Teller;

    /// A counter is made at its first record, on the meter installed then:
    /// one first recorded once a provider is installed counts, though another
    /// of its policy recorded before, and that other counts nothing, after
    /// the provider is installed too.
    #[test]
    fn a_counter_reports_only_when_its_first_record_found_a_meter_provider_installed() {
        let counters = Counters::new("quotes");
        counters.retry();
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        counters.retry();
        counters.fallback();

        provider.force_flush().expect("the export");
        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let reported: Vec<_> = exported
            .iter()
            .flat_map(|export| export.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .map(|metric| metric.name())
            .collect();
        assert_eq!(
            reported,
            [FALLBACKS],
            "the fallbacks, first recorded since, and no retry"
        );
    }

    /// A counter panicking at every record, as a meter's pipeline may.
    struct PanickingCounter;

    impl opentelemetry::metrics::SyncInstrument<u64> for PanickingCounter {
        fn measure(&self, _count: u64, _attributes: &[opentelemetry::KeyValue]) {
            panic!("the meter fails");
        }
    }

    /// A counter panicking as a transition is counted unwinds what moved the
    /// breaker, as a line does: the breaker is in the phase it entered, and a
    /// probe whose send it unwound hands its turn on — from a destructor as
    /// the thread unwinds, where the second panic ends.
    #[tokio::test(start_paused = true)]
    async fn a_counter_panicking_out_of_a_transition_leaves_the_breaker_moved() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        use crate::BreakerState;

        const BOUND: Duration = Duration::from_secs(5);
        const OPEN_FOR: Duration = Duration::from_secs(30);

        let counters = Counters::new("quotes");
        let panicking = opentelemetry::metrics::Counter::new(Arc::new(PanickingCounter));
        counters
            .transitions
            .set(panicking)
            .expect("no transition was counted yet");
        let settings = BreakerSettings {
            window: 1,
            failures: 1,
            open_for: OPEN_FOR,
            age: None,
            probes: 1,
        };
        let breaker = Breaker::new(settings, Arc::new(Teller::counting_on("quotes", counters)));

        let sent = breaker
            .admit(BOUND)
            .expect("a closed breaker admits")
            .send(BOUND, None);
        let failing = catch_unwind(AssertUnwindSafe(|| sent.settle(Some("down"))));
        assert!(failing.is_err(), "the opening's count");
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            }
        );

        tokio::time::advance(OPEN_FOR).await;
        let probe = breaker.admit(BOUND).expect("the open time passed");
        let probing = catch_unwind(AssertUnwindSafe(|| probe.send(BOUND, None).settle(None)));
        assert!(probing.is_err(), "the half-opening's count");
        assert!(
            breaker.admit(BOUND).is_ok(),
            "the probe's turn is free for the next call"
        );
    }
}
