//! The circuit breaker a policy keeps: a count window over its latest
//! attempts, opened by enough failures in it, and half-open for one probe once
//! its open time has passed. Its state lives in the process.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::counters::Counters;
use crate::{TARGET, millis};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Closed,
    Open {
        until: Instant,
    },
    /// One call probes the dependency; every other is refused until it answers.
    HalfOpen,
}

impl Phase {
    /// The word a line and the counter say.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open { .. } => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// The phase and the window of latest outcomes, `true` for a failure.
#[derive(Debug)]
struct Held {
    phase: Phase,
    outcomes: VecDeque<bool>,
    failed: usize,
}

impl Held {
    fn forget(&mut self) {
        self.outcomes.clear();
        self.failed = 0;
    }
}

pub(crate) struct Breaker {
    instance: &'static str,
    window: usize,
    failures: usize,
    open_for: Duration,
    /// What a call refused while the probe runs is told to wait: the probe's
    /// own bound, an attempt's timeout.
    probe_wait: Duration,
    held: Mutex<Held>,
    counters: Arc<Counters>,
}

/// One admitted attempt, settled once with its outcome. Dropped unsettled —
/// the caller gave the call up — it counts for nothing, and a probe hands its
/// turn to the next call.
pub(crate) struct Pass<'b> {
    breaker: &'b Breaker,
    probe: bool,
}

impl Pass<'_> {
    pub(crate) fn settle(mut self, failed: bool) {
        let probe = std::mem::take(&mut self.probe);
        self.breaker.record(probe, failed);
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if self.probe {
            self.breaker.abandon_probe();
        }
    }
}

impl Breaker {
    /// A closed breaker for `instance`, opening once `failures` of the latest
    /// `window` attempts failed, for `open_for`; a call refused during a probe
    /// is told to wait `probe_wait`. Each transition counts on `counters`.
    pub(crate) fn new(
        instance: &'static str,
        window: usize,
        failures: usize,
        open_for: Duration,
        probe_wait: Duration,
        counters: Arc<Counters>,
    ) -> Self {
        Self {
            instance,
            window,
            failures,
            open_for,
            probe_wait,
            held: Mutex::new(Held {
                phase: Phase::Closed,
                outcomes: VecDeque::with_capacity(window),
                failed: 0,
            }),
            counters,
        }
    }

    /// Let a call through, or refuse it for the wait the breaker knows.
    pub(crate) fn admit(&self) -> Result<Pass<'_>, Duration> {
        let mut held = self.held.lock();
        if let Some(wait) = self.refused_for(held.phase) {
            return Err(wait);
        }
        let probe = matches!(held.phase, Phase::Open { .. });
        if probe {
            held.phase = Phase::HalfOpen;
            drop(held);
            self.entered(Phase::HalfOpen, None);
        }
        Ok(Pass {
            breaker: self,
            probe,
        })
    }

    /// How long a call is refused for now, leaving the breaker as it stands.
    pub(crate) fn refusing(&self) -> Option<Duration> {
        self.refused_for(self.held.lock().phase)
    }

    /// The wait a call meeting `phase` is refused for: `None` when it would
    /// pass, or probe.
    fn refused_for(&self, phase: Phase) -> Option<Duration> {
        match phase {
            Phase::Closed => None,
            Phase::HalfOpen => Some(self.probe_wait),
            Phase::Open { until } => until
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero()),
        }
    }

    #[cfg(test)]
    fn state(&self) -> &'static str {
        self.held.lock().phase.as_str()
    }

    /// Count an attempt's outcome: a probe closes or reopens the breaker, any
    /// other attempt counts in the window while it is closed — and for nothing
    /// once another call opened it.
    fn record(&self, probe: bool, failed: bool) {
        let mut held = self.held.lock();
        let entered = if probe {
            if failed {
                held.phase = self.opened();
                Some((held.phase, Some(1)))
            } else {
                held.phase = Phase::Closed;
                held.forget();
                Some((Phase::Closed, None))
            }
        } else if held.phase == Phase::Closed {
            if held.outcomes.len() == self.window && held.outcomes.pop_front() == Some(true) {
                held.failed -= 1;
            }
            held.outcomes.push_back(failed);
            held.failed += usize::from(failed);
            (held.failed >= self.failures).then(|| {
                let counted = held.failed;
                held.phase = self.opened();
                held.forget();
                (held.phase, Some(counted))
            })
        } else {
            None
        };
        drop(held);
        if let Some((phase, failed)) = entered {
            self.entered(phase, failed);
        }
    }

    /// A probe given up before it answered: the next call probes instead.
    fn abandon_probe(&self) {
        let mut held = self.held.lock();
        if held.phase == Phase::HalfOpen {
            let reopened = Phase::Open {
                until: Instant::now(),
            };
            held.phase = reopened;
            drop(held);
            self.entered(reopened, None);
        }
    }

    fn opened(&self) -> Phase {
        Phase::Open {
            until: Instant::now() + self.open_for,
        }
    }

    /// Say the transition into `phase`, and count it; `failed` is how many
    /// failures in the window opened it, when failures did.
    fn entered(&self, phase: Phase, failed: Option<usize>) {
        tracing::warn!(
            target: TARGET,
            instance = self.instance,
            state = phase.as_str(),
            failed,
            window = failed.map(|_| self.window),
            open_ms = failed.map(|_| millis(self.open_for)),
            "circuit breaker changed state",
        );
        self.counters.transition(phase.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN_FOR: Duration = Duration::from_secs(30);
    const PROBE_WAIT: Duration = Duration::from_secs(5);

    fn breaker(window: usize, failures: usize) -> Breaker {
        Breaker::new(
            "upstream",
            window,
            failures,
            OPEN_FOR,
            PROBE_WAIT,
            Arc::new(Counters::new("upstream")),
        )
    }

    fn attempt(breaker: &Breaker, failed: bool) {
        breaker
            .admit()
            .expect("a closed breaker admits")
            .settle(failed);
    }

    fn refused_for(breaker: &Breaker) -> Duration {
        match breaker.admit() {
            Err(retry_after) => retry_after,
            Ok(_) => panic!("the breaker refuses"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn it_opens_once_the_window_holds_enough_failures() {
        let breaker = breaker(4, 2);
        attempt(&breaker, true);
        attempt(&breaker, false);
        assert_eq!(breaker.state(), "closed", "one failure of two");
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
        assert_eq!(refused_for(&breaker), OPEN_FOR);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_older_than_the_window_counts_no_more() {
        let breaker = breaker(2, 2);
        attempt(&breaker, true);
        attempt(&breaker, false);
        attempt(&breaker, false);
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure left the window before the second came",
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_says_how_long_is_left_then_lets_one_probe_through() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(refused_for(&breaker), Duration::from_secs(20));
        tokio::time::advance(Duration::from_secs(20)).await;
        let Ok(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        assert_eq!(breaker.state(), "half_open");
        assert_eq!(
            refused_for(&breaker),
            PROBE_WAIT,
            "one probe at a time; another call waits for its answer",
        );
        probe.settle(false);
        assert_eq!(breaker.state(), "closed");
        attempt(&breaker, false);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_opens_the_breaker_again_for_its_whole_open_time() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        probe.settle(true);
        assert_eq!(breaker.state(), "open");
        assert_eq!(refused_for(&breaker), OPEN_FOR);
    }

    #[tokio::test(start_paused = true)]
    async fn a_probe_given_up_hands_its_turn_to_the_next_call() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        drop(probe);
        assert_eq!(breaker.state(), "open");
        let Ok(next) = breaker.admit() else {
            panic!("the next call probes at once");
        };
        next.settle(false);
        assert_eq!(breaker.state(), "closed");
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_arriving_once_another_call_opened_the_breaker_counts_for_nothing() {
        let breaker = breaker(1, 1);
        let Ok(late) = breaker.admit() else {
            panic!("closed");
        };
        attempt(&breaker, true);
        late.settle(false);
        assert_eq!(breaker.state(), "open", "a late success closes nothing");
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_breaker_starts_its_window_afresh() {
        let breaker = breaker(3, 2);
        attempt(&breaker, true);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        probe.settle(false);
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the failures before it opened are forgotten",
        );
    }

    /// Each transition adds one to the counter, by the instance and the state it
    /// entered, on the meter `OpenTelemetry::init()` installs — after the
    /// breaker was built, too.
    #[cfg(feature = "opentelemetry")]
    #[tokio::test(start_paused = true)]
    async fn each_transition_is_counted_by_its_instance_and_the_state_entered() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};

        use crate::counters::{INSTANCE_ATTRIBUTE, STATE_ATTRIBUTE, TRANSITIONS};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        let breaker = breaker(1, 1);
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        probe.settle(true);
        provider.force_flush().expect("the export");

        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let mut counted: Vec<(String, u64)> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == TRANSITIONS)
            .flat_map(|metric| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    panic!("a counter: {metric:?}");
                };
                sum.data_points()
                    .map(|point| {
                        let attribute = |key: &str| {
                            point
                                .attributes()
                                .find(|kv| kv.key.as_str() == key)
                                .map(|kv| kv.value.to_string())
                                .unwrap_or_default()
                        };
                        assert_eq!(attribute(INSTANCE_ATTRIBUTE), "upstream");
                        (attribute(STATE_ATTRIBUTE), point.value())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        counted.sort();
        assert_eq!(
            counted,
            [("half_open".to_owned(), 1), ("open".to_owned(), 2)],
            "opened, probed, opened again"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn each_transition_is_a_warn_naming_the_instance_and_the_state_entered() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        probe.settle(false);
        let said: Vec<(String, String)> = lines
            .find(TARGET, "circuit breaker changed state")
            .iter()
            .map(|event| {
                assert_eq!(event.level, "warn", "{event:?}");
                assert_eq!(event.field("instance").as_deref(), Some("upstream"));
                (
                    event.field("state").unwrap_or_default(),
                    event.field("open_ms").unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                ("open".to_owned(), "30000".to_owned()),
                ("half_open".to_owned(), String::new()),
                ("closed".to_owned(), String::new()),
            ]
        );
    }
}
