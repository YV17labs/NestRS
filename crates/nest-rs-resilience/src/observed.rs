//! [`Observed`] — what a policy holds now, observed on the meter: its
//! breaker's state and window, its bulkhead's calls running and waiting, its
//! rate's tokens free, read when the metric reader exports and recorded on no
//! run. Without the `opentelemetry` feature nothing is observed.

use std::sync::Weak;

use crate::mechanisms::Mechanisms;

/// The breaker's state as a call arriving now meets it: `1` on the state it
/// is in, `0` on each other.
#[cfg(feature = "opentelemetry")]
pub(crate) const BREAKER_STATUS: &str = "nestrs.resilience.breaker.status";

/// The outcomes the breaker's window counts now, by whether each is a failure
/// or a success.
#[cfg(feature = "opentelemetry")]
pub(crate) const BREAKER_OUTCOMES: &str = "nestrs.resilience.breaker.outcomes";

/// The calls holding a slot of the bulkhead now.
#[cfg(feature = "opentelemetry")]
pub(crate) const CONCURRENCY_RUNNING: &str = "nestrs.resilience.concurrency.running";

/// The calls waiting for a slot of the bulkhead now.
#[cfg(feature = "opentelemetry")]
pub(crate) const CONCURRENCY_WAITING: &str = "nestrs.resilience.concurrency.waiting";

/// The tokens of the rate limit free now.
#[cfg(feature = "opentelemetry")]
pub(crate) const RATE_AVAILABLE: &str = "nestrs.resilience.rate.available";

/// `failure` or `success`, on the outcomes a breaker's window counts.
#[cfg(feature = "opentelemetry")]
pub(crate) const OUTCOME_ATTRIBUTE: &str = "nestrs.resilience.breaker.outcome";

/// The meter provider `opentelemetry::global` hands out.
#[cfg(feature = "opentelemetry")]
type Provider = std::sync::Arc<dyn opentelemetry::metrics::MeterProvider + Send + Sync>;

/// What a policy's reads are observed off — its mechanisms, held weakly, so
/// an instrument left on a meter never keeps a policy past its app — and
/// where: on the meter provider installed as the policy is built, on the one
/// installed by its first run when one came between the two, and on each one
/// `OpenTelemetry::init()` seats after, from the run following it.
pub(crate) struct Observed {
    #[cfg(feature = "opentelemetry")]
    reads: Reads,
    /// The provider the reads were registered on last.
    #[cfg(feature = "opentelemetry")]
    registered_on: parking_lot::Mutex<Provider>,
    /// How many providers were seated when a run last compared the one
    /// installed with it: none read yet, before the first run.
    #[cfg(feature = "opentelemetry")]
    compared_at: std::sync::atomic::AtomicU64,
}

/// What [`Observed::compared_at`] holds before the first run.
#[cfg(feature = "opentelemetry")]
const NOT_COMPARED: u64 = u64::MAX;

impl Observed {
    /// The reads of the policy of `instance` off `mechanisms`, of each of its
    /// breaker, its bulkhead and its rate that is on, observed on the meter
    /// provider installed now.
    pub(crate) fn new(
        instance: &'static str,
        mechanisms: &Weak<Mechanisms>,
        breaker: bool,
        bulkhead: bool,
        rate: bool,
    ) -> Self {
        #[cfg(not(feature = "opentelemetry"))]
        let _ = (instance, mechanisms, breaker, bulkhead, rate);
        #[cfg(feature = "opentelemetry")]
        {
            let reads = Reads {
                instance,
                mechanisms: mechanisms.clone(),
                breaker,
                bulkhead,
                rate,
            };
            let installed = opentelemetry::global::meter_provider();
            reads.observe(installed.as_ref());
            Self {
                reads,
                registered_on: parking_lot::Mutex::new(installed),
                compared_at: std::sync::atomic::AtomicU64::new(NOT_COMPARED),
            }
        }
        #[cfg(not(feature = "opentelemetry"))]
        Self {}
    }

    /// Called as a run opens: the policy's first one, and the first after
    /// `OpenTelemetry::init()` seats a meter provider, observe its reads on
    /// the provider installed then, when it is not the one they are on. Every
    /// other run reads two counts.
    #[inline]
    pub(crate) fn at_run(&self) {
        #[cfg(feature = "opentelemetry")]
        {
            let seated = nest_rs_core::__private::meter_providers_seated();
            if self.compared_at.load(std::sync::atomic::Ordering::Relaxed) != seated {
                self.compare(seated);
            }
        }
    }

    #[cfg(feature = "opentelemetry")]
    #[cold]
    fn compare(&self, seated: u64) {
        // Under the lock, so two runs racing on one seat observe once.
        let mut registered_on = self.registered_on.lock();
        if self.compared_at.load(std::sync::atomic::Ordering::Relaxed) == seated {
            return;
        }
        let installed = opentelemetry::global::meter_provider();
        if !std::sync::Arc::ptr_eq(&registered_on, &installed) {
            self.reads.observe(installed.as_ref());
            *registered_on = installed;
        }
        self.compared_at
            .store(seated, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The mechanisms of one policy, held weakly, which of them are on, and the
/// instance its instruments say.
#[cfg(feature = "opentelemetry")]
struct Reads {
    instance: &'static str,
    mechanisms: Weak<Mechanisms>,
    breaker: bool,
    bulkhead: bool,
    rate: bool,
}

#[cfg(feature = "opentelemetry")]
impl Reads {
    /// Register on `provider`'s meter the instruments of the reads of each
    /// mechanism the policy has on — one a read, the breaker's window as one
    /// instrument by outcome — each observed when the metric reader exports,
    /// and observing nothing once the policy is dropped.
    ///
    /// An export runs these reads holding the meter's pipeline, which a
    /// counter's first record waits for: each takes one lock of its mechanism
    /// at most, for the read alone, and no counter records under one.
    fn observe(&self, provider: &(dyn opentelemetry::metrics::MeterProvider + Send + Sync)) {
        use opentelemetry::KeyValue;

        use crate::breaker::BreakerPhase;
        use crate::counters::{INSTANCE_ATTRIBUTE, STATE_ATTRIBUTE};

        let meter = provider.meter("nestrs");
        let instance = self.instance;
        if self.breaker {
            let observed = self.mechanisms.clone();
            meter
                .i64_observable_up_down_counter(BREAKER_STATUS)
                .with_unit("1")
                .with_description(
                    "The circuit breakers' state as a call arriving now meets it: 1 on the \
                     state a breaker is in, 0 on each other.",
                )
                .with_callback(move |observer| {
                    let Some(mechanisms) = observed.upgrade() else {
                        return;
                    };
                    let Some(breaker) = &mechanisms.breaker else {
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
            let observed = self.mechanisms.clone();
            meter
                .i64_observable_up_down_counter(BREAKER_OUTCOMES)
                .with_unit("{outcome}")
                .with_description(
                    "The outcomes the circuit breakers' windows count now, by whether each is a \
                     failure or a success.",
                )
                .with_callback(move |observer| {
                    let Some(mechanisms) = observed.upgrade() else {
                        return;
                    };
                    let Some(breaker) = &mechanisms.breaker else {
                        return;
                    };
                    for (outcome, count) in by_outcome(breaker.stats()) {
                        observer.observe(
                            signed(count),
                            &[
                                KeyValue::new(INSTANCE_ATTRIBUTE, instance),
                                KeyValue::new(OUTCOME_ATTRIBUTE, outcome),
                            ],
                        );
                    }
                })
                .build();
        }
        if self.bulkhead {
            observe(
                &meter,
                CONCURRENCY_RUNNING,
                "{call}",
                "The calls holding a slot of the policies' bulkheads now.",
                instance,
                &self.mechanisms,
                |mechanisms| {
                    mechanisms
                        .bulkhead
                        .as_ref()
                        .map(crate::bulkhead::Bulkhead::running)
                },
            );
            observe(
                &meter,
                CONCURRENCY_WAITING,
                "{call}",
                "The calls waiting for a slot of the policies' bulkheads now.",
                instance,
                &self.mechanisms,
                |mechanisms| {
                    mechanisms
                        .bulkhead
                        .as_ref()
                        .map(crate::bulkhead::Bulkhead::waiting)
                },
            );
        }
        if self.rate {
            observe(
                &meter,
                RATE_AVAILABLE,
                "{token}",
                "The tokens of the policies' rate limits free now.",
                instance,
                &self.mechanisms,
                |mechanisms| {
                    mechanisms
                        .rate
                        .as_ref()
                        .map(|rate| rate.available(tokio::time::Instant::now()))
                },
            );
        }
    }
}

/// One count `read` off `mechanisms` when the metric reader exports, by
/// `instance`, and not at all once their policy is dropped.
#[cfg(feature = "opentelemetry")]
fn observe(
    meter: &opentelemetry::metrics::Meter,
    name: &'static str,
    unit: &'static str,
    description: &'static str,
    instance: &'static str,
    mechanisms: &Weak<Mechanisms>,
    read: impl Fn(&Mechanisms) -> Option<usize> + Send + Sync + 'static,
) {
    let observed = mechanisms.clone();
    meter
        .i64_observable_up_down_counter(name)
        .with_unit(unit)
        .with_description(description)
        .with_callback(move |observer| {
            if let Some(count) = observed.upgrade().as_deref().and_then(&read) {
                observer.observe(
                    signed(count),
                    &[opentelemetry::KeyValue::new(
                        crate::counters::INSTANCE_ATTRIBUTE,
                        instance,
                    )],
                );
            }
        })
        .build();
}

/// The two series of a breaker's window, both off one read of it: read
/// apart, an export could pair the failures of one instant with the successes
/// of another.
#[cfg(feature = "opentelemetry")]
const fn by_outcome(stats: crate::breaker::BreakerStats) -> [(&'static str, usize); 2] {
    [
        ("failure", stats.failures),
        ("success", stats.outcomes.saturating_sub(stats.failures)),
    ]
}

/// `count` as an up-down counter observes it.
#[cfg(feature = "opentelemetry")]
fn signed(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[cfg(feature = "opentelemetry")]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Waker};
    use std::time::Duration;

    use nest_rs_core::{Instance, instance};
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;
    use crate::counters::{INSTANCE_ATTRIBUTE, STATE_ATTRIBUTE, TRANSITIONS};
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

    /// One point an instrument observed: the instrument, the instance, what
    /// the instrument is keyed by beside it — the breaker's state on its
    /// status, the outcome on its window's — and the value.
    type Point = (&'static str, &'static str, &'static str, i64);

    /// Every point of the instruments the reads are observed on, as the last
    /// export says them.
    fn observed(exporter: &InMemoryMetricExporter) -> Vec<(String, String, String, i64)> {
        const OBSERVED: [&str; 5] = [
            BREAKER_STATUS,
            BREAKER_OUTCOMES,
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
                        };
                        let by = match metric.name() {
                            BREAKER_STATUS => attribute(STATE_ATTRIBUTE),
                            BREAKER_OUTCOMES => attribute(OUTCOME_ATTRIBUTE),
                            _ => None,
                        };
                        (
                            metric.name().to_owned(),
                            attribute(INSTANCE_ATTRIBUTE).unwrap_or_default(),
                            by.unwrap_or_default(),
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

    /// A policy's first transition makes the transitions' counter, which waits
    /// for the meter's pipeline; an export holds that pipeline while it runs
    /// each observed read, the breaker's among them. The transition lands
    /// while the export is inside an app's own read, registered first: both
    /// end, the counter made with no lock of the breaker held. The app's read
    /// lasts until the opening has had time to reach the pipeline; a runner
    /// stalled past that only spares the test the race.
    #[test]
    fn a_first_transition_racing_an_export_completes() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        let (collecting, in_collect) = std::sync::mpsc::channel();
        let (starting, started) = std::sync::mpsc::channel();
        let started = parking_lot::Mutex::new(Some(started));
        let _app = opentelemetry::global::meter("app")
            .i64_observable_gauge("app.slow")
            .with_callback(move |_| {
                let Some(started) = started.lock().take() else {
                    return;
                };
                collecting.send(()).expect("the test waits for the export");
                if started.recv_timeout(Duration::from_secs(5)).is_ok() {
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
            .build();
        let policy = Arc::new(policy::<Quotes>(ResilienceConfig::default()));
        let flushing = {
            let provider = provider.clone();
            std::thread::spawn(move || provider.force_flush())
        };
        in_collect.recv().expect("the export runs the app's read");

        let (opened, done) = std::sync::mpsc::channel();
        let opening = {
            let policy = Arc::clone(&policy);
            std::thread::spawn(move || {
                starting.send(()).expect("the export waits for the opening");
                policy
                    .open_breaker(Duration::from_secs(60))
                    .expect("a breaker");
                opened.send(()).expect("the test waits for the opening");
            })
        };
        let finished = done.recv_timeout(Duration::from_secs(5)).is_ok();
        if !finished {
            // Joined, or shut down, what waits here would hang the test past its verdict.
            std::mem::forget((provider, flushing, opening, policy));
            panic!("the first transition and the export wait for each other");
        }
        flushing
            .join()
            .expect("the export's thread")
            .expect("the export");
        opening.join().expect("the opening's thread");

        exporter.reset();
        provider.force_flush().expect("the export");
        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        assert!(
            exported
                .iter()
                .flat_map(|export| export.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .any(|metric| metric.name() == TRANSITIONS),
            "the transition counted, its counter made while the export ran"
        );
        assert!(
            observed(&exporter).contains(&(
                BREAKER_STATUS.to_owned(),
                "quotes".to_owned(),
                "open".to_owned(),
                1
            )),
            "and the breaker is read open"
        );
    }

    /// The reads are observed on the meter by instance, read when the reader
    /// exports and recorded on no run: a policy built before the meter
    /// provider was installed reports from its first run, one built after it
    /// with no run at all, a mechanism that is off reports nothing, and a
    /// policy dropped reports no more. Each instrument is read at two values
    /// at least, so none exports a constant: the breaker's status on each
    /// state, its window with failures and successes, calls running and one
    /// waiting, the rate under its limit. The clock is the real one, which a
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
            concurrency_wait: Some(Duration::from_secs(60)),
            rate_limit: Some(5),
            ..ResilienceConfig::default()
        });

        provider.force_flush().expect("the export");
        assert_eq!(
            observed(&exporter),
            sorted(&[
                (BREAKER_OUTCOMES, "carrier", "failure", 0),
                (BREAKER_OUTCOMES, "carrier", "success", 0),
                (BREAKER_STATUS, "carrier", "closed", 1),
                (BREAKER_STATUS, "carrier", "half_open", 0),
                (BREAKER_STATUS, "carrier", "open", 0),
                (CONCURRENCY_RUNNING, "carrier", "", 0),
                (CONCURRENCY_WAITING, "carrier", "", 0),
                (RATE_AVAILABLE, "carrier", "", 5),
            ]),
            "the policy built since reports with no run, the one built before not yet"
        );

        assert!(before.run(|| async { Err::<(), _>(Down) }).await.is_err());
        for _ in 0..2 {
            assert!(before.run(|| async { Ok::<_, Down>(()) }).await.is_ok());
        }
        let holding = parked(before.run(silent));
        assert!(after.run(|| async { Ok::<_, Down>(()) }).await.is_ok());
        let sent = [parked(after.run(silent)), parked(after.run(silent))];
        let waiting = parked(after.run(silent));
        after
            .open_breaker(Duration::from_secs(60))
            .expect("a breaker");
        exporter.reset();
        provider.force_flush().expect("the export");
        assert_eq!(
            observed(&exporter),
            sorted(&[
                (BREAKER_OUTCOMES, "quotes", "failure", 1),
                (BREAKER_OUTCOMES, "quotes", "success", 2),
                (BREAKER_STATUS, "quotes", "closed", 1),
                (BREAKER_STATUS, "quotes", "half_open", 0),
                (BREAKER_STATUS, "quotes", "open", 0),
                (CONCURRENCY_RUNNING, "quotes", "", 1),
                (CONCURRENCY_WAITING, "quotes", "", 0),
                (BREAKER_OUTCOMES, "carrier", "failure", 0),
                (BREAKER_OUTCOMES, "carrier", "success", 0),
                (BREAKER_STATUS, "carrier", "closed", 0),
                (BREAKER_STATUS, "carrier", "half_open", 0),
                (BREAKER_STATUS, "carrier", "open", 1),
                (CONCURRENCY_RUNNING, "carrier", "", 2),
                (CONCURRENCY_WAITING, "carrier", "", 1),
                (RATE_AVAILABLE, "carrier", "", 2),
            ]),
            "the one built before from its first run, its window's three outcomes by what each \
             was, with no rate to report; the carrier's three attempts sent, two still running \
             and a fourth call waiting for a slot, its window forgotten as it was opened by hand"
        );

        const OPEN_FOR: Duration = Duration::from_millis(1);
        after.open_breaker(OPEN_FOR).expect("a breaker");
        // Past the open time, however long the runner makes it.
        tokio::time::sleep(OPEN_FOR * 5).await;
        exporter.reset();
        provider.force_flush().expect("the export");
        let status: Vec<_> = observed(&exporter)
            .into_iter()
            .filter(|(name, instance, ..)| name == BREAKER_STATUS && instance == "carrier")
            .collect();
        assert_eq!(
            status,
            sorted(&[
                (BREAKER_STATUS, "carrier", "closed", 0),
                (BREAKER_STATUS, "carrier", "half_open", 1),
                (BREAKER_STATUS, "carrier", "open", 0),
            ]),
            "its open time passed, the next call its probe"
        );

        drop((holding, sent, waiting));
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

    /// A policy whose first run came before `OpenTelemetry::init()` seated
    /// the meter provider reports its reads there from its next run, where
    /// its first run's comparison was its last and it reported nothing, then
    /// or after.
    #[tokio::test]
    async fn the_reads_of_a_policy_run_before_its_meter_provider_was_seated_report_once_it_is() {
        let early = policy::<Quotes>(ResilienceConfig {
            retries: None,
            concurrency: Some(3),
            ..ResilienceConfig::default()
        });
        assert!(early.run(|| async { Ok::<_, Down>(()) }).await.is_ok());
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        nest_rs_core::__private::meter_provider_seated();

        assert!(early.run(|| async { Ok::<_, Down>(()) }).await.is_ok());
        provider.force_flush().expect("the export");
        assert_eq!(
            observed(&exporter),
            sorted(&[
                (BREAKER_OUTCOMES, "quotes", "failure", 0),
                (BREAKER_OUTCOMES, "quotes", "success", 2),
                (BREAKER_STATUS, "quotes", "closed", 1),
                (BREAKER_STATUS, "quotes", "half_open", 0),
                (BREAKER_STATUS, "quotes", "open", 0),
                (CONCURRENCY_RUNNING, "quotes", "", 0),
                (CONCURRENCY_WAITING, "quotes", "", 0),
            ]),
            "observed from the run after the seat"
        );
    }
}
