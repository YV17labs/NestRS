//! The circuit breaker a policy keeps: a count window over its latest
//! attempts, opened by enough failures in it, and half-open while one probe's
//! call is out once its open time has passed. Its state lives in the process.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::counters::Counters;
use crate::{TARGET, millis};

/// What a call refused beside a probe past its bound is told: one tick of the
/// timer, the probe being given up at any moment.
const OVERDUE: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Closed,
    Open {
        until: Instant,
    },
    /// The open time passed and one call holds the probe's turn, its call not
    /// sent yet: every other call is refused until the probe has answered, by
    /// `until` at the latest, and the breaker reads as open, the dependency
    /// not called.
    ProbePending {
        until: Instant,
    },
    /// One call probes the dependency; every other is refused until it
    /// answers, by `until` at the latest.
    HalfOpen {
        until: Instant,
    },
}

impl Phase {
    /// The word a line and the counter say.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open { .. } | Self::ProbePending { .. } => "open",
            Self::HalfOpen { .. } => "half_open",
        }
    }

    /// The wait a call meeting the breaker in this phase is refused for:
    /// `None` when it would pass, or probe.
    fn refused_for(self) -> Option<Duration> {
        match self {
            Self::Closed => None,
            Self::Open { until } => until
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero()),
            Self::ProbePending { until } | Self::HalfOpen { until } => {
                Some(until.saturating_duration_since(Instant::now()).max(OVERDUE))
            }
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
    held: Mutex<Held>,
    counters: Arc<Counters>,
}

/// One attempt the breaker admitted, its call not sent yet. Dropped so — the
/// rate refused it, the unit ran out of time, its caller gave it up — it
/// counts for nothing, and a probe hands its turn to the next call with no
/// transition, the dependency never called.
pub(crate) struct Pass<'b> {
    breaker: &'b Breaker,
    probe: bool,
}

impl<'b> Pass<'b> {
    /// Whether the attempt holds the probe's turn.
    pub(crate) const fn probes(&self) -> bool {
        self.probe
    }

    /// The attempt's call postponed to `to`, then sent under `bound`: a probe
    /// has answered by their sum at the latest, which every call the breaker
    /// refuses meanwhile is told.
    pub(crate) fn postpone(&self, to: Instant, bound: Duration) {
        if self.probe {
            self.breaker.probe_postponed(to + bound);
        }
    }

    /// The attempt's call sent under `bound`: a probe's makes the breaker
    /// half-open until it answers, within `bound`.
    pub(crate) fn send(mut self, bound: Duration) -> Sent<'b> {
        let probe = std::mem::take(&mut self.probe);
        if probe {
            self.breaker.probe_sent(bound);
        }
        Sent {
            breaker: self.breaker,
            probe,
        }
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if self.probe {
            self.breaker.withdraw_probe();
        }
    }
}

/// One admitted attempt whose call was sent, settled once with its outcome.
/// Dropped unsettled — the caller gave the call up — it counts for nothing,
/// and a probe opens the breaker again for the next call to probe.
pub(crate) struct Sent<'b> {
    breaker: &'b Breaker,
    probe: bool,
}

impl Sent<'_> {
    pub(crate) fn settle(mut self, failed: bool) {
        let probe = std::mem::take(&mut self.probe);
        self.breaker.record(probe, failed);
    }
}

impl Drop for Sent<'_> {
    fn drop(&mut self) {
        if self.probe {
            self.breaker.abandon_probe();
        }
    }
}

impl Breaker {
    /// A closed breaker for `instance`, opening once `failures` of the latest
    /// `window` attempts failed, for `open_for`. Each transition counts on
    /// `counters`.
    pub(crate) fn new(
        instance: &'static str,
        window: usize,
        failures: usize,
        open_for: Duration,
        counters: Arc<Counters>,
    ) -> Self {
        Self {
            instance,
            window,
            failures,
            open_for,
            held: Mutex::new(Held {
                phase: Phase::Closed,
                outcomes: VecDeque::with_capacity(window),
                failed: 0,
            }),
            counters,
        }
    }

    /// Let a call through — taking the probe's turn once the open time has
    /// passed, its call sent now under `bound` unless it postpones it — or
    /// refuse it for the wait the breaker knows: what is left of the open
    /// time, or of the probe's own bound while another call holds its turn.
    pub(crate) fn admit(&self, bound: Duration) -> Result<Pass<'_>, Duration> {
        let mut held = self.held.lock();
        if let Some(wait) = held.phase.refused_for() {
            return Err(wait);
        }
        let probe = matches!(held.phase, Phase::Open { .. });
        if probe {
            held.phase = Phase::ProbePending {
                until: Instant::now() + bound,
            };
        }
        Ok(Pass {
            breaker: self,
            probe,
        })
    }

    /// How long a call is refused for now, leaving the breaker as it stands.
    pub(crate) fn refusing(&self) -> Option<Duration> {
        self.held.lock().phase.refused_for()
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

    /// The probe's call postponed: it has answered by `until` at the latest.
    fn probe_postponed(&self, until: Instant) {
        if let Phase::ProbePending { until: latest } = &mut self.held.lock().phase {
            *latest = until;
        }
    }

    /// The probe's call sent under `bound`: the breaker is half-open until it
    /// answers.
    fn probe_sent(&self, bound: Duration) {
        let mut held = self.held.lock();
        if matches!(held.phase, Phase::ProbePending { .. }) {
            let half_open = Phase::HalfOpen {
                until: Instant::now() + bound,
            };
            held.phase = half_open;
            drop(held);
            self.entered(half_open, None);
        }
    }

    /// A probe given up before its call was sent: the next call probes
    /// instead, and the breaker, open all along, files no transition.
    fn withdraw_probe(&self) {
        let mut held = self.held.lock();
        if matches!(held.phase, Phase::ProbePending { .. }) {
            held.phase = Phase::Open {
                until: Instant::now(),
            };
        }
    }

    /// A probe given up after its call was sent, before it answered: the next
    /// call probes instead.
    fn abandon_probe(&self) {
        let mut held = self.held.lock();
        if matches!(held.phase, Phase::HalfOpen { .. }) {
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
    /// What each call is sent under, but where a test sets its own.
    const BOUND: Duration = Duration::from_secs(5);

    fn breaker(window: usize, failures: usize) -> Breaker {
        Breaker::new(
            "upstream",
            window,
            failures,
            OPEN_FOR,
            Arc::new(Counters::new("upstream")),
        )
    }

    fn attempt(breaker: &Breaker, failed: bool) {
        breaker
            .admit(BOUND)
            .expect("a closed breaker admits")
            .send(BOUND)
            .settle(failed);
    }

    fn states(lines: &nest_rs_testing::LogCapture) -> Vec<String> {
        lines
            .find(TARGET, "circuit breaker changed state")
            .iter()
            .map(|event| event.field("state").unwrap_or_default())
            .collect()
    }

    fn refused_for(breaker: &Breaker) -> Duration {
        match breaker.admit(BOUND) {
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
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        assert_eq!(
            (breaker.state(), refused_for(&breaker)),
            ("open", BOUND),
            "the probe's turn is taken, its call not sent yet",
        );
        let probe = probe.send(BOUND);
        assert_eq!(breaker.state(), "half_open");
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "one probe at a time; another call waits for its answer",
        );
        probe.settle(false);
        assert_eq!(breaker.state(), "closed");
        attempt(&breaker, false);
    }

    /// A call refused while another holds the probe's turn is told what is
    /// left of the bound that probe handed the breaker — when it took its
    /// turn, when it postponed its call, and when it sent it — and, past it,
    /// a tick: the probe is being given up, and still holds its turn.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_beside_the_probe_is_told_what_is_left_of_the_probes_own_bound() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let second = Duration::from_secs(1);
        let probe = breaker.admit(2 * second).expect("the open time passed");
        assert_eq!(refused_for(&breaker), 2 * second, "sent now, under 2 s");
        probe.postpone(Instant::now() + 10 * second, 2 * second);
        assert_eq!(
            refused_for(&breaker),
            12 * second,
            "sent in 10 s, under 2 s"
        );
        tokio::time::advance(10 * second).await;
        let probe = probe.send(3 * second);
        assert_eq!(refused_for(&breaker), 3 * second, "sent, under 3 s");
        tokio::time::advance(second).await;
        assert_eq!(refused_for(&breaker), 2 * second);
        tokio::time::advance(5 * second).await;
        assert_eq!(refused_for(&breaker), OVERDUE);
        probe.settle(false);
        assert_eq!(breaker.state(), "closed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_opens_the_breaker_again_for_its_whole_open_time() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND).settle(true);
        assert_eq!(breaker.state(), "open");
        assert_eq!(refused_for(&breaker), OPEN_FOR);
    }

    /// A probe given up before its call is sent hands its turn on as though it
    /// never took it, filing nothing; one given up after it, the breaker open
    /// again for the next call to probe, files that transition.
    #[tokio::test(start_paused = true)]
    async fn a_probe_given_up_hands_its_turn_to_the_next_call() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(breaker.admit(BOUND).expect("the open time passed"));
        assert_eq!(breaker.state(), "open");
        let sent = breaker
            .admit(BOUND)
            .expect("the next call probes at once")
            .send(BOUND);
        drop(sent);
        assert_eq!(breaker.state(), "open");
        attempt(&breaker, false);
        assert_eq!(breaker.state(), "closed", "the next call probed again");
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "closed"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_arriving_once_another_call_opened_the_breaker_counts_for_nothing() {
        let breaker = breaker(1, 1);
        let Ok(late) = breaker.admit(BOUND) else {
            panic!("closed");
        };
        let late = late.send(BOUND);
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
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND).settle(false);
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
        drop(
            breaker
                .admit(BOUND)
                .expect("a probe given up before its call is sent"),
        );
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND).settle(true);
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
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND).settle(false);
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
