//! The circuit breaker a policy keeps: a count window over its latest
//! attempts, opened by enough failures in it, and half-open for one probe once
//! its open time has passed. Its state lives in the process.

use std::collections::VecDeque;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::TARGET;

/// The counter each transition adds one to, by the state it entered.
#[cfg(feature = "opentelemetry")]
pub(crate) const TRANSITIONS: &str = "nestrs.resilience.breaker.transitions";

/// The attribute naming the policy's instance on [`TRANSITIONS`].
#[cfg(feature = "opentelemetry")]
pub(crate) const INSTANCE_ATTRIBUTE: &str = "nestrs.resilience.instance";

/// The attribute naming the state a transition entered on [`TRANSITIONS`].
#[cfg(feature = "opentelemetry")]
pub(crate) const STATE_ATTRIBUTE: &str = "nestrs.resilience.breaker.state";

/// A breaker's state, as its lines and its counter say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Every call passes, and its outcome counts in the window.
    Closed,
    /// Every call is refused until the open time has passed.
    Open,
    /// One call probes the dependency; every other is refused until it answers.
    HalfOpen,
}

impl State {
    /// The word a line and the counter say.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// Where the breaker stands.
#[derive(Clone, Copy, Debug)]
enum Phase {
    Closed,
    Open { until: Instant },
    Probing,
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

/// A count-window circuit breaker.
pub(crate) struct Breaker {
    instance: &'static str,
    window: usize,
    failures: usize,
    open_for: Duration,
    /// What a call refused while the probe runs is told to wait: the probe's
    /// own bound, an attempt's timeout.
    probe_wait: Duration,
    held: Mutex<Held>,
    #[cfg(feature = "opentelemetry")]
    transitions: opentelemetry::metrics::Counter<u64>,
}

/// Whether the breaker lets a call through.
pub(crate) enum Admission<'b> {
    /// The call may run; its outcome is settled through the pass.
    Admitted(Pass<'b>),
    /// The call is refused; the breaker lets one through again after
    /// `retry_after` at the earliest.
    Refused { retry_after: Duration },
}

/// One admitted attempt, settled once with its outcome. Dropped unsettled —
/// the caller gave the call up — it counts for nothing, and a probe hands its
/// turn to the next call.
pub(crate) struct Pass<'b> {
    breaker: &'b Breaker,
    probe: bool,
    settled: bool,
}

impl Pass<'_> {
    /// The attempt succeeded, or `failed`.
    pub(crate) fn settle(mut self, failed: bool) {
        self.settled = true;
        self.breaker.record(self.probe, failed);
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if !self.settled && self.probe {
            self.breaker.abandon_probe();
        }
    }
}

impl Breaker {
    /// A closed breaker for `instance`, opening once `failures` of the latest
    /// `window` attempts failed, for `open_for`; a call refused during a probe
    /// is told to wait `probe_wait`.
    pub(crate) fn new(
        instance: &'static str,
        window: usize,
        failures: usize,
        open_for: Duration,
        probe_wait: Duration,
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
            #[cfg(feature = "opentelemetry")]
            transitions: opentelemetry::global::meter("nestrs")
                .u64_counter(TRANSITIONS)
                .with_unit("{transition}")
                .with_description("The circuit breakers' transitions, by the state each entered.")
                .build(),
        }
    }

    /// Let a call through, or refuse it.
    pub(crate) fn admit(&self) -> Admission<'_> {
        let mut held = self.held.lock();
        match held.phase {
            Phase::Closed => Admission::Admitted(self.pass(false)),
            Phase::Probing => Admission::Refused {
                retry_after: self.probe_wait,
            },
            Phase::Open { until } => {
                let now = Instant::now();
                if now < until {
                    return Admission::Refused {
                        retry_after: until - now,
                    };
                }
                held.phase = Phase::Probing;
                drop(held);
                self.entered(State::HalfOpen, None);
                Admission::Admitted(self.pass(true))
            }
        }
    }

    /// How long a call is refused for now, leaving the breaker as it stands:
    /// `None` when the next call would pass, or probe.
    pub(crate) fn refusing(&self) -> Option<Duration> {
        match self.held.lock().phase {
            Phase::Closed => None,
            Phase::Probing => Some(self.probe_wait),
            Phase::Open { until } => until
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero()),
        }
    }

    /// Where the breaker stands now.
    #[cfg(test)]
    pub(crate) fn state(&self) -> State {
        match self.held.lock().phase {
            Phase::Closed => State::Closed,
            Phase::Open { .. } => State::Open,
            Phase::Probing => State::HalfOpen,
        }
    }

    fn pass(&self, probe: bool) -> Pass<'_> {
        Pass {
            breaker: self,
            probe,
            settled: false,
        }
    }

    /// Count an attempt's outcome: a probe closes or reopens the breaker, any
    /// other attempt counts in the window while it is closed — and for nothing
    /// once another call opened it.
    fn record(&self, probe: bool, failed: bool) {
        let mut held = self.held.lock();
        let entered = if probe {
            if failed {
                held.phase = self.opened();
                Some((State::Open, 1))
            } else {
                held.phase = Phase::Closed;
                held.forget();
                Some((State::Closed, 0))
            }
        } else if matches!(held.phase, Phase::Closed) {
            held.outcomes.push_back(failed);
            held.failed += usize::from(failed);
            if held.outcomes.len() > self.window && held.outcomes.pop_front() == Some(true) {
                held.failed -= 1;
            }
            if held.failed >= self.failures {
                let counted = held.failed;
                held.phase = self.opened();
                held.forget();
                Some((State::Open, counted))
            } else {
                None
            }
        } else {
            None
        };
        drop(held);
        if let Some((state, failed)) = entered {
            self.entered(state, Some(failed));
        }
    }

    /// A probe given up before it answered: the next call probes instead.
    fn abandon_probe(&self) {
        let mut held = self.held.lock();
        if matches!(held.phase, Phase::Probing) {
            held.phase = Phase::Open {
                until: Instant::now(),
            };
            drop(held);
            self.entered(State::Open, None);
        }
    }

    fn opened(&self) -> Phase {
        Phase::Open {
            until: Instant::now() + self.open_for,
        }
    }

    /// Say the transition into `state`, and count it; `failed` is how many
    /// failures in the window decided it.
    fn entered(&self, state: State, failed: Option<usize>) {
        match (state, failed) {
            (State::Open, Some(failed)) => tracing::warn!(
                target: TARGET,
                instance = self.instance,
                state = state.as_str(),
                failed,
                window = self.window,
                open_ms = u64::try_from(self.open_for.as_millis()).unwrap_or(u64::MAX),
                "circuit breaker changed state",
            ),
            _ => tracing::warn!(
                target: TARGET,
                instance = self.instance,
                state = state.as_str(),
                "circuit breaker changed state",
            ),
        }
        #[cfg(feature = "opentelemetry")]
        self.transitions.add(
            1,
            &[
                opentelemetry::KeyValue::new(INSTANCE_ATTRIBUTE, self.instance),
                opentelemetry::KeyValue::new(STATE_ATTRIBUTE, state.as_str()),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN_FOR: Duration = Duration::from_secs(30);
    const PROBE_WAIT: Duration = Duration::from_secs(5);

    fn breaker(window: usize, failures: usize) -> Breaker {
        Breaker::new("upstream", window, failures, OPEN_FOR, PROBE_WAIT)
    }

    fn attempt(breaker: &Breaker, failed: bool) {
        match breaker.admit() {
            Admission::Admitted(pass) => pass.settle(failed),
            Admission::Refused { .. } => panic!("a closed breaker admits"),
        }
    }

    fn refused_for(breaker: &Breaker) -> Duration {
        match breaker.admit() {
            Admission::Refused { retry_after } => retry_after,
            Admission::Admitted(_) => panic!("the breaker refuses"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn it_opens_once_the_window_holds_enough_failures() {
        let breaker = breaker(4, 2);
        attempt(&breaker, true);
        attempt(&breaker, false);
        assert_eq!(breaker.state(), State::Closed, "one failure of two");
        attempt(&breaker, true);
        assert_eq!(breaker.state(), State::Open);
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
            State::Closed,
            "the first failure left the window before the second came",
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), State::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_says_how_long_is_left_then_lets_one_probe_through() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(refused_for(&breaker), Duration::from_secs(20));
        tokio::time::advance(Duration::from_secs(20)).await;
        let Admission::Admitted(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        assert_eq!(breaker.state(), State::HalfOpen);
        assert_eq!(
            refused_for(&breaker),
            PROBE_WAIT,
            "one probe at a time; another call waits for its answer",
        );
        probe.settle(false);
        assert_eq!(breaker.state(), State::Closed);
        attempt(&breaker, false);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_opens_the_breaker_again_for_its_whole_open_time() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Admission::Admitted(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        probe.settle(true);
        assert_eq!(breaker.state(), State::Open);
        assert_eq!(refused_for(&breaker), OPEN_FOR);
    }

    #[tokio::test(start_paused = true)]
    async fn a_probe_given_up_hands_its_turn_to_the_next_call() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Admission::Admitted(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        drop(probe);
        assert_eq!(breaker.state(), State::Open);
        let Admission::Admitted(next) = breaker.admit() else {
            panic!("the next call probes at once");
        };
        next.settle(false);
        assert_eq!(breaker.state(), State::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_arriving_once_another_call_opened_the_breaker_counts_for_nothing() {
        let breaker = breaker(1, 1);
        let Admission::Admitted(late) = breaker.admit() else {
            panic!("closed");
        };
        attempt(&breaker, true);
        late.settle(false);
        assert_eq!(
            breaker.state(),
            State::Open,
            "a late success closes nothing"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_breaker_starts_its_window_afresh() {
        let breaker = breaker(3, 2);
        attempt(&breaker, true);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Admission::Admitted(probe) = breaker.admit() else {
            panic!("the open time passed");
        };
        probe.settle(false);
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            State::Closed,
            "the failures before it opened are forgotten",
        );
    }

    /// Each transition adds one to the counter, by the instance and the state it
    /// entered, on the meter `OpenTelemetry::init()` installs.
    #[cfg(feature = "opentelemetry")]
    #[tokio::test(start_paused = true)]
    async fn each_transition_is_counted_by_its_instance_and_the_state_entered() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Admission::Admitted(probe) = breaker.admit() else {
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
        let Admission::Admitted(probe) = breaker.admit() else {
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
