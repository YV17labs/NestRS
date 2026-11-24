//! The counters a policy records on — its breaker's transitions, its retries,
//! the refusals its runs end on and the fallbacks answering in place of a
//! run's error — each made on the global meter at its first use, so a policy
//! built before `OpenTelemetry::init()` still counts; and the ones its reads
//! are [`Observed`] on, read when the metric reader exports and recorded on no
//! run. Without the `opentelemetry` feature a record is nothing, and nothing
//! is observed.

use std::sync::Arc;

use crate::breaker::Breaker;
use crate::bulkhead::Bulkhead;
use crate::error::Refusal;
use crate::rate::Rate;

/// Each transition of a breaker, by the state it entered.
#[cfg(feature = "opentelemetry")]
pub(crate) const TRANSITIONS: &str = "nestrs.resilience.breaker.transitions";

/// Each call a policy sent again.
#[cfg(feature = "opentelemetry")]
pub(crate) const RETRIES: &str = "nestrs.resilience.retries";

/// Each run a policy ended on its own refusal, by the refusal.
#[cfg(feature = "opentelemetry")]
pub(crate) const REFUSALS: &str = "nestrs.resilience.refusals";

/// Each run whose fallback answered in place of its error.
#[cfg(feature = "opentelemetry")]
pub(crate) const FALLBACKS: &str = "nestrs.resilience.fallbacks";

/// The breaker's state as a call arriving now meets it: `1` on the state it
/// is in, `0` on each other.
#[cfg(feature = "opentelemetry")]
pub(crate) const BREAKER_STATUS: &str = "nestrs.resilience.breaker.status";

/// The outcomes the breaker's window counts now.
#[cfg(feature = "opentelemetry")]
pub(crate) const BREAKER_OUTCOMES: &str = "nestrs.resilience.breaker.outcomes";

/// The failures among the outcomes the breaker's window counts now.
#[cfg(feature = "opentelemetry")]
pub(crate) const BREAKER_FAILURES: &str = "nestrs.resilience.breaker.failures";

/// The calls holding a slot of the bulkhead now.
#[cfg(feature = "opentelemetry")]
pub(crate) const CONCURRENCY_RUNNING: &str = "nestrs.resilience.concurrency.running";

/// The calls waiting for a slot of the bulkhead now.
#[cfg(feature = "opentelemetry")]
pub(crate) const CONCURRENCY_WAITING: &str = "nestrs.resilience.concurrency.waiting";

/// The tokens of the rate limit free now.
#[cfg(feature = "opentelemetry")]
pub(crate) const RATE_AVAILABLE: &str = "nestrs.resilience.rate.available";

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

/// A counter made at its first use.
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

/// The counter `lazy` holds, made on the global meter at its first use.
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

/// The meter provider `opentelemetry::global` hands out.
#[cfg(feature = "opentelemetry")]
type Provider = Arc<dyn opentelemetry::metrics::MeterProvider + Send + Sync>;

/// What a policy's reads are observed off — its breaker, its bulkhead and its
/// rate, each held weakly, so an instrument left on a meter never keeps a
/// policy past its app — and where: on the meter provider installed as the
/// policy is built, and on the one installed by its first run when
/// `OpenTelemetry::init()` came between the two.
pub(crate) struct Observed {
    #[cfg(feature = "opentelemetry")]
    reads: Reads,
    /// The provider the reads were registered on as the policy was built,
    /// until its first run compared it with the one installed then.
    #[cfg(feature = "opentelemetry")]
    built_on: parking_lot::Mutex<Option<Provider>>,
    #[cfg(feature = "opentelemetry")]
    compared: std::sync::atomic::AtomicBool,
}

impl Observed {
    /// The reads of the policy of `instance`, over the mechanisms it has on,
    /// observed on the meter provider installed now.
    pub(crate) fn new(
        instance: &'static str,
        breaker: Option<&Arc<Breaker>>,
        bulkhead: Option<&Arc<Bulkhead>>,
        rate: Option<&Arc<Rate>>,
    ) -> Self {
        #[cfg(not(feature = "opentelemetry"))]
        let _ = (instance, breaker, bulkhead, rate);
        #[cfg(feature = "opentelemetry")]
        {
            let reads = Reads {
                instance,
                breaker: breaker.map(Arc::downgrade),
                bulkhead: bulkhead.map(Arc::downgrade),
                rate: rate.map(Arc::downgrade),
            };
            let installed = opentelemetry::global::meter_provider();
            reads.observe(installed.as_ref());
            Self {
                reads,
                built_on: parking_lot::Mutex::new(Some(installed)),
                compared: std::sync::atomic::AtomicBool::new(false),
            }
        }
        #[cfg(not(feature = "opentelemetry"))]
        Self {}
    }

    /// Called as a run opens: the policy's first one observes its reads on the
    /// meter provider installed since the policy was built, when one was.
    /// Every later run reads one flag.
    #[inline]
    pub(crate) fn at_run(&self) {
        #[cfg(feature = "opentelemetry")]
        if !self.compared.load(std::sync::atomic::Ordering::Relaxed) {
            self.compare();
        }
    }

    #[cfg(feature = "opentelemetry")]
    #[cold]
    fn compare(&self) {
        // Taken by one run alone, so two racing on the first observe once.
        let Some(built_on) = self.built_on.lock().take() else {
            return;
        };
        let installed = opentelemetry::global::meter_provider();
        if !Arc::ptr_eq(&built_on, &installed) {
            self.reads.observe(installed.as_ref());
        }
        self.compared
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The mechanisms of one policy, held weakly, and the instance its
/// instruments say.
#[cfg(feature = "opentelemetry")]
struct Reads {
    instance: &'static str,
    breaker: Option<std::sync::Weak<Breaker>>,
    bulkhead: Option<std::sync::Weak<Bulkhead>>,
    rate: Option<std::sync::Weak<Rate>>,
}

#[cfg(feature = "opentelemetry")]
impl Reads {
    /// Register on `provider`'s meter one instrument per read of a mechanism
    /// the policy has on, each observed when the metric reader exports, and
    /// observing nothing once the policy is dropped.
    fn observe(&self, provider: &(dyn opentelemetry::metrics::MeterProvider + Send + Sync)) {
        use opentelemetry::KeyValue;

        use crate::breaker::BreakerPhase;

        let meter = provider.meter("nestrs");
        let instance = self.instance;
        if let Some(breaker) = &self.breaker {
            let observed = breaker.clone();
            meter
                .i64_observable_up_down_counter(BREAKER_STATUS)
                .with_unit("1")
                .with_description(
                    "The circuit breakers' state as a call arriving now meets it: 1 on the \
                     state a breaker is in, 0 on each other.",
                )
                .with_callback(move |observer| {
                    let Some(breaker) = observed.upgrade() else {
                        return;
                    };
                    let state = breaker.read().as_str();
                    for phase in [
                        BreakerPhase::Closed,
                        BreakerPhase::Open,
                        BreakerPhase::HalfOpen,
                    ] {
                        observer.observe(
                            i64::from(phase.as_str() == state),
                            &[
                                KeyValue::new(INSTANCE_ATTRIBUTE, instance),
                                KeyValue::new(STATE_ATTRIBUTE, phase.as_str()),
                            ],
                        );
                    }
                })
                .build();
            observe(
                &meter,
                BREAKER_OUTCOMES,
                "{outcome}",
                "The outcomes the circuit breakers' windows count now.",
                instance,
                breaker,
                |breaker| breaker.stats().outcomes,
            );
            observe(
                &meter,
                BREAKER_FAILURES,
                "{failure}",
                "The failures among the outcomes the circuit breakers' windows count now.",
                instance,
                breaker,
                |breaker| breaker.stats().failures,
            );
        }
        if let Some(bulkhead) = &self.bulkhead {
            observe(
                &meter,
                CONCURRENCY_RUNNING,
                "{call}",
                "The calls holding a slot of the policies' bulkheads now.",
                instance,
                bulkhead,
                Bulkhead::running,
            );
            observe(
                &meter,
                CONCURRENCY_WAITING,
                "{call}",
                "The calls waiting for a slot of the policies' bulkheads now.",
                instance,
                bulkhead,
                Bulkhead::waiting,
            );
        }
        if let Some(rate) = &self.rate {
            observe(
                &meter,
                RATE_AVAILABLE,
                "{token}",
                "The tokens of the policies' rate limits free now.",
                instance,
                rate,
                |rate| rate.available(tokio::time::Instant::now()),
            );
        }
    }
}

/// One count `read` off `mechanism` when the metric reader exports, by
/// `instance`, and not at all once the mechanism's policy is dropped.
#[cfg(feature = "opentelemetry")]
fn observe<M: Send + Sync + 'static>(
    meter: &opentelemetry::metrics::Meter,
    name: &'static str,
    unit: &'static str,
    description: &'static str,
    instance: &'static str,
    mechanism: &std::sync::Weak<M>,
    read: impl Fn(&M) -> usize + Send + Sync + 'static,
) {
    let observed = mechanism.clone();
    meter
        .i64_observable_up_down_counter(name)
        .with_unit(unit)
        .with_description(description)
        .with_callback(move |observer| {
            if let Some(mechanism) = observed.upgrade() {
                observer.observe(
                    i64::try_from(read(&mechanism)).unwrap_or(i64::MAX),
                    &[opentelemetry::KeyValue::new(INSTANCE_ATTRIBUTE, instance)],
                );
            }
        })
        .build();
}

#[cfg(test)]
#[cfg(feature = "opentelemetry")]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Waker};
    use std::time::Duration;

    use nest_rs_core::{Instance, instance};
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;
    use crate::{ResilienceConfig, ResiliencePolicy};

    #[instance]
    struct Quotes;

    #[instance]
    struct Carrier;

    #[derive(Debug, thiserror::Error)]
    #[error("the dependency is down")]
    struct Down;

    fn policy<I: Instance>(config: ResilienceConfig) -> ResiliencePolicy<I> {
        ResiliencePolicy::new(&config).expect("a policy its settings allow")
    }

    /// `run` polled once, and still waiting on its call.
    fn parked<'f, T>(run: impl Future<Output = T> + 'f) -> Pin<Box<dyn Future<Output = T> + 'f>> {
        let mut run: Pin<Box<dyn Future<Output = T> + 'f>> = Box::pin(run);
        assert!(
            run.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        run
    }

    fn silent() -> std::future::Pending<Result<(), Down>> {
        std::future::pending()
    }

    /// One point an instrument observed: the instrument, the instance, the
    /// breaker's state where the point carries one, and the value.
    type Point = (&'static str, &'static str, &'static str, i64);

    /// Every point of the instruments the reads are observed on, as the last
    /// export says them.
    fn observed(exporter: &InMemoryMetricExporter) -> Vec<(String, String, String, i64)> {
        const OBSERVED: [&str; 6] = [
            BREAKER_STATUS,
            BREAKER_OUTCOMES,
            BREAKER_FAILURES,
            CONCURRENCY_RUNNING,
            CONCURRENCY_WAITING,
            RATE_AVAILABLE,
        ];
        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let last = exported.last().expect("an export");
        let mut points: Vec<_> = last
            .scope_metrics()
            .flat_map(|scope| scope.metrics())
            .filter(|metric| OBSERVED.contains(&metric.name()))
            .flat_map(|metric| {
                let AggregatedMetrics::I64(MetricData::Sum(sum)) = metric.data() else {
                    panic!("an up-down counter: {metric:?}");
                };
                assert!(!sum.is_monotonic(), "{}", metric.name());
                sum.data_points()
                    .map(|point| {
                        let attribute = |key: &str| {
                            point
                                .attributes()
                                .find(|kv| kv.key.as_str() == key)
                                .map(|kv| kv.value.to_string())
                                .unwrap_or_default()
                        };
                        (
                            metric.name().to_owned(),
                            attribute(INSTANCE_ATTRIBUTE),
                            attribute(STATE_ATTRIBUTE),
                            point.value(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        points.sort();
        points
    }

    fn sorted(points: &[Point]) -> Vec<(String, String, String, i64)> {
        let mut points: Vec<_> = points
            .iter()
            .map(|&(name, instance, state, value)| {
                (
                    name.to_owned(),
                    instance.to_owned(),
                    state.to_owned(),
                    value,
                )
            })
            .collect();
        points.sort();
        points
    }

    /// The reads are observed on the meter by instance, read when the reader
    /// exports and recorded on no run: a policy built before the meter
    /// provider was installed reports from its first run, one built after it
    /// with no run at all, a mechanism that is off reports nothing, and a
    /// policy dropped reports no more. The clock is the real one, which a
    /// reader's thread reads.
    #[tokio::test]
    async fn the_reads_are_observed_on_the_meter_whenever_its_provider_was_installed() {
        let before = policy::<Quotes>(ResilienceConfig {
            retries: None,
            concurrency: Some(3),
            ..ResilienceConfig::default()
        });
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        let after = policy::<Carrier>(ResilienceConfig {
            retries: None,
            concurrency: Some(2),
            rate_limit: Some(5),
            ..ResilienceConfig::default()
        });

        provider.force_flush().expect("the export");
        assert_eq!(
            observed(&exporter),
            sorted(&[
                (BREAKER_FAILURES, "carrier", "", 0),
                (BREAKER_OUTCOMES, "carrier", "", 0),
                (BREAKER_STATUS, "carrier", "closed", 1),
                (BREAKER_STATUS, "carrier", "half_open", 0),
                (BREAKER_STATUS, "carrier", "open", 0),
                (CONCURRENCY_RUNNING, "carrier", "", 0),
                (CONCURRENCY_WAITING, "carrier", "", 0),
                (RATE_AVAILABLE, "carrier", "", 5),
            ]),
            "the policy built since reports with no run, the one built before not yet"
        );

        assert!(before.run(|_| async { Err::<(), _>(Down) }).await.is_err());
        let holding = parked(before.run(|_| silent()));
        assert!(after.run(|_| async { Ok::<_, Down>(()) }).await.is_ok());
        let sent = parked(after.run(|_| silent()));
        after
            .open_breaker(Duration::from_secs(60))
            .expect("a breaker");
        exporter.reset();
        provider.force_flush().expect("the export");
        assert_eq!(
            observed(&exporter),
            sorted(&[
                (BREAKER_FAILURES, "quotes", "", 1),
                (BREAKER_OUTCOMES, "quotes", "", 1),
                (BREAKER_STATUS, "quotes", "closed", 1),
                (BREAKER_STATUS, "quotes", "half_open", 0),
                (BREAKER_STATUS, "quotes", "open", 0),
                (CONCURRENCY_RUNNING, "quotes", "", 1),
                (CONCURRENCY_WAITING, "quotes", "", 0),
                (BREAKER_FAILURES, "carrier", "", 0),
                (BREAKER_OUTCOMES, "carrier", "", 0),
                (BREAKER_STATUS, "carrier", "closed", 0),
                (BREAKER_STATUS, "carrier", "half_open", 0),
                (BREAKER_STATUS, "carrier", "open", 1),
                (CONCURRENCY_RUNNING, "carrier", "", 1),
                (CONCURRENCY_WAITING, "carrier", "", 0),
                (RATE_AVAILABLE, "carrier", "", 3),
            ]),
            "the one built before from its first run, with no rate to report; the carrier's \
             two attempts sent, one still running, its window forgotten as it was opened by hand"
        );

        drop((holding, sent));
        drop(after);
        exporter.reset();
        provider.force_flush().expect("the export");
        assert!(
            observed(&exporter)
                .iter()
                .all(|(_, instance, ..)| instance == "quotes"),
            "the carrier's policy is gone, and its instruments observe nothing"
        );
    }
}
