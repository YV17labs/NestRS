//! The circuit breaker a policy keeps: a count window over its latest
//! attempts, forgetting an outcome past its age when outcomes have one, opened
//! by enough failures in it, probing the dependency one call at a time once
//! its open time has passed, closed by enough probes in a row, and opened or
//! closed by hand. Its state lives in the process.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard};
use tokio::time::Instant;

use crate::TARGET;
use crate::counters::Counters;
use crate::millis::millis;

/// What a call refused beside a probe past its bound is told: one tick of the
/// timer, the probe being given up at any moment.
const OVERDUE: Duration = Duration::from_millis(1);

/// A policy's circuit breaker, as the next call meets it:
/// [`ResiliencePolicy::breaker_state`](crate::ResiliencePolicy::breaker_state).
///
/// It says what the breaker does to a call arriving now, where the `warn` each
/// transition files says what it does to the dependency: once the open time
/// has passed the read is [`HalfOpen`](Self::HalfOpen), and the line says
/// `half_open` only when a probe's call is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakerState {
    /// The call is sent, and its outcome counts in the window.
    Closed,
    /// The call is refused unsent, as long as the open time lasts.
    #[non_exhaustive]
    Open {
        /// What is left of the open time, never zero: the wait a call refused
        /// now is told, as [`ResilienceError::Open`](crate::ResilienceError::Open)
        /// says it.
        retry_after: Duration,
    },
    /// The open time has passed, and the breaker lets one probe through at a
    /// time. The call takes the probe's turn when it is free — the open time
    /// over, or between two probes of the series
    /// [`breaker_probes`](crate::ResilienceConfig::breaker_probes) asks — and
    /// is refused while another call holds it, whether that call is sent yet
    /// or still waits for the rate's reset, told when the probe has answered
    /// at the latest. It carries no time: that bound is the probe's own run's.
    HalfOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Closed,
    Open {
        until: Instant,
    },
    /// The open time passed and one call holds the probe's turn, its call not
    /// sent yet, `passed` probes of the series having succeeded before it:
    /// every other call is refused until the probe has answered, by `until`
    /// at the latest, and the lines read the breaker as open until a probe of
    /// the series is sent, the dependency not called.
    ProbePending {
        until: Instant,
        passed: usize,
    },
    /// One call probes the dependency, `passed` probes of the series having
    /// succeeded before it; every other is refused until it answers, by
    /// `until` at the latest.
    HalfOpen {
        until: Instant,
        passed: usize,
    },
    /// Between two probes of a series, `passed` of them having succeeded: the
    /// probe's turn is free, and the next call takes it.
    BetweenProbes {
        passed: usize,
    },
}

impl Phase {
    /// The word a line and the counter say.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open { .. } | Self::ProbePending { passed: 0, .. } => "open",
            Self::ProbePending { .. } | Self::HalfOpen { .. } | Self::BetweenProbes { .. } => {
                "half_open"
            }
        }
    }

    /// What a call meeting the breaker in this phase gets: let through —
    /// sent as the breaker is closed (`None`), or taking the probe's turn
    /// after the `passed` probes of its series that succeeded — or refused for
    /// the wait the breaker knows.
    fn admitting(self) -> Result<Option<usize>, Duration> {
        match self {
            Self::Closed => Ok(None),
            Self::Open { until } => left(until).map_or(Ok(Some(0)), Err),
            Self::BetweenProbes { passed } => Ok(Some(passed)),
            Self::ProbePending { until, .. } | Self::HalfOpen { until, .. } => {
                Err(until.saturating_duration_since(Instant::now()).max(OVERDUE))
            }
        }
    }

    /// The state a call arriving now meets.
    fn state(self) -> BreakerState {
        match self {
            Self::Closed => BreakerState::Closed,
            Self::Open { until } => left(until).map_or(BreakerState::HalfOpen, |retry_after| {
                BreakerState::Open { retry_after }
            }),
            Self::ProbePending { .. } | Self::HalfOpen { .. } | Self::BetweenProbes { .. } => {
                BreakerState::HalfOpen
            }
        }
    }
}

/// What is left of an open time ending at `until`, `None` once it has passed.
fn left(until: Instant) -> Option<Duration> {
    until
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

/// The latest attempts' outcomes, oldest first, `true` for a failure — beside
/// the instant each was counted at only when outcomes have an age, so a
/// breaker without one stores no instant and reads no clock to count.
#[derive(Debug)]
enum Outcomes {
    Counted(VecDeque<bool>),
    Aged {
        age: Duration,
        counted: VecDeque<(Instant, bool)>,
    },
}

/// The phase, the window of latest outcomes and how many of them failed, and
/// the epoch: how many manual switches came, each voiding what the attempts
/// admitted before it would change.
#[derive(Debug)]
struct Held {
    phase: Phase,
    outcomes: Outcomes,
    failed: usize,
    epoch: u64,
}

impl Held {
    fn forget(&mut self) {
        match &mut self.outcomes {
            Outcomes::Counted(outcomes) => outcomes.clear(),
            Outcomes::Aged { counted, .. } => counted.clear(),
        }
        self.failed = 0;
    }

    /// Count an outcome among the latest `window`, once those older than the
    /// age are forgotten: how many failed. `now` is read only where outcomes
    /// have an age.
    fn count(&mut self, window: usize, failed: bool, now: impl FnOnce() -> Instant) -> usize {
        match &mut self.outcomes {
            Outcomes::Counted(outcomes) => {
                if outcomes.len() == window && outcomes.pop_front() == Some(true) {
                    self.failed -= 1;
                }
                outcomes.push_back(failed);
            }
            Outcomes::Aged { age, counted } => {
                let now = now();
                while let Some(&(at, old)) = counted.front()
                    && now.saturating_duration_since(at) > *age
                {
                    counted.pop_front();
                    self.failed -= usize::from(old);
                }
                if counted.len() == window && counted.pop_front().is_some_and(|(_, old)| old) {
                    self.failed -= 1;
                }
                counted.push_back((now, failed));
            }
        }
        self.failed += usize::from(failed);
        self.failed
    }
}

pub(crate) struct Breaker {
    instance: &'static str,
    window: usize,
    failures: usize,
    open_for: Duration,
    probes: usize,
    held: Mutex<Held>,
    counters: Arc<Counters>,
}

/// One attempt the breaker admitted, its call not sent yet. Dropped so — the
/// rate refused it, the unit ran out of time, its caller dropped it — it
/// counts for nothing, and a probe hands its turn to the next call with no
/// transition, the dependency never called. It carries the epoch it was
/// admitted in, so a manual switch since leaves it nothing to change.
pub(crate) struct Pass<'b> {
    breaker: &'b Breaker,
    probe: bool,
    epoch: u64,
}

impl<'b> Pass<'b> {
    /// Whether the attempt holds the probe's turn: it took it, and no manual
    /// switch came since.
    pub(crate) fn probes(&self) -> bool {
        self.probe && self.breaker.held.lock().epoch == self.epoch
    }

    /// The attempt's call postponed to `to`, then sent under `bound`: a probe
    /// has answered by their sum at the latest, which every call the breaker
    /// refuses meanwhile is told.
    pub(crate) fn postpone(&self, to: Instant, bound: Duration) {
        if self.probe {
            self.breaker.probe_postponed(self.epoch, to + bound);
        }
    }

    /// The attempt's call sent under `bound`, which ends at `until` when the
    /// attempt's timer is armed at an instant: a probe's makes the breaker
    /// half-open until it answers, within `bound`.
    pub(crate) fn send(mut self, bound: Duration, until: Option<Instant>) -> Sent<'b> {
        let probe = std::mem::take(&mut self.probe);
        if probe {
            self.breaker.probe_sent(self.epoch, bound);
        }
        Sent {
            breaker: self.breaker,
            probe,
            epoch: self.epoch,
            until,
        }
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if self.probe {
            self.breaker.withdraw_probe(self.epoch);
        }
    }
}

/// One admitted attempt whose call was sent, settled once with its outcome.
/// Dropped unsettled before its bound ends — its caller dropped the call — it
/// counts for nothing, and a probe hands its turn to the next call. Dropped
/// unsettled at or past the instant its bound ends at, it has timed out, as
/// its own timer due there would have said: an edge cutting its unit at that
/// deadline may be polled first, tokio firing one instant's timers in batches
/// that another worker runs between.
pub(crate) struct Sent<'b> {
    breaker: &'b Breaker,
    probe: bool,
    epoch: u64,
    /// The instant the attempt's bound ends at, when its timer is armed at one.
    until: Option<Instant>,
}

impl Sent<'_> {
    pub(crate) fn settle(mut self, failed: bool) {
        let probe = std::mem::take(&mut self.probe);
        self.until = None;
        self.breaker.record(self.epoch, probe, failed);
    }
}

impl Drop for Sent<'_> {
    fn drop(&mut self) {
        if self.until.is_some_and(|until| Instant::now() >= until) {
            self.breaker.record(self.epoch, self.probe, true);
        } else if self.probe {
            self.breaker.abandon_probe(self.epoch);
        }
    }
}

/// What moved the breaker, as the line its transition files says it.
#[derive(Clone, Copy)]
enum Moved {
    /// Its attempts' outcomes: `failed` failures counted, when failures
    /// opened it.
    Attempts { failed: Option<usize> },
    /// A manual switch, opening it for `open_for` or closing it.
    Manual { open_for: Option<Duration> },
}

impl Breaker {
    /// A closed breaker for `instance`, opening for `open_for` once `failures`
    /// of the latest `window` attempts failed, and closing once `probes`
    /// probes in a row succeeded. With an `age`, an outcome counts until it is
    /// older than that, its own age running from its answer. Each transition
    /// counts on `counters`.
    pub(crate) fn new(
        instance: &'static str,
        window: usize,
        failures: usize,
        open_for: Duration,
        age: Option<Duration>,
        probes: usize,
        counters: Arc<Counters>,
    ) -> Self {
        let outcomes = match age {
            None => Outcomes::Counted(VecDeque::with_capacity(window)),
            Some(age) => Outcomes::Aged {
                age,
                counted: VecDeque::with_capacity(window),
            },
        };
        Self {
            instance,
            window,
            failures,
            open_for,
            probes,
            held: Mutex::new(Held {
                phase: Phase::Closed,
                outcomes,
                failed: 0,
                epoch: 0,
            }),
            counters,
        }
    }

    /// Let a call through — taking the probe's turn once the open time has
    /// passed or between two probes of a series, its call sent now under
    /// `bound` unless it postpones it — or refuse it for the wait the breaker
    /// knows: what is left of the open time, or of the probe's own bound while
    /// another call holds its turn.
    pub(crate) fn admit(&self, bound: Duration) -> Result<Pass<'_>, Duration> {
        let mut held = self.held.lock();
        let probing = held.phase.admitting()?;
        if let Some(passed) = probing {
            held.phase = Phase::ProbePending {
                until: Instant::now() + bound,
                passed,
            };
        }
        Ok(Pass {
            breaker: self,
            probe: probing.is_some(),
            epoch: held.epoch,
        })
    }

    /// How long a call is refused for now, leaving the breaker as it stands.
    pub(crate) fn refusing(&self) -> Option<Duration> {
        self.held.lock().phase.admitting().err()
    }

    /// The state a call arriving now meets, leaving the breaker as it stands.
    pub(crate) fn read(&self) -> BreakerState {
        self.held.lock().phase.state()
    }

    #[cfg(test)]
    fn state(&self) -> &'static str {
        self.held.lock().phase.as_str()
    }

    /// Open the breaker by hand for `open_for` from now, whatever it was in:
    /// every call is refused that long, then probed as after any opening, and
    /// no attempt admitted before changes it.
    pub(crate) fn open_manually(&self, open_for: Duration) {
        self.switch(
            Phase::Open {
                until: Instant::now() + open_for,
            },
            Some(open_for),
        );
    }

    /// Close the breaker by hand, whatever it was in, its window forgotten,
    /// and no attempt admitted before changing it.
    pub(crate) fn close_manually(&self) {
        self.switch(Phase::Closed, None);
    }

    /// Enter `phase` by hand, in an epoch of its own, opened for `open_for`
    /// when it opens.
    fn switch(&self, phase: Phase, open_for: Option<Duration>) {
        let mut held = self.held.lock();
        held.epoch = held.epoch.wrapping_add(1);
        held.phase = phase;
        held.forget();
        self.entered(&held, Moved::Manual { open_for });
    }

    /// The breaker's state for an attempt admitted in `epoch`: `None` once a
    /// manual switch came since, which leaves the attempt nothing to change.
    fn held_since(&self, epoch: u64) -> Option<MutexGuard<'_, Held>> {
        let held = self.held.lock();
        (held.epoch == epoch).then_some(held)
    }

    /// Count the outcome of an attempt admitted in `epoch`: a probe opens the
    /// breaker again, or closes it as the last of its series, and any other
    /// attempt counts in the window while it is closed — and for nothing once
    /// another call opened it.
    fn record(&self, epoch: u64, probe: bool, failed: bool) {
        let Some(mut held) = self.held_since(epoch) else {
            return;
        };
        if probe {
            let Phase::HalfOpen { passed, .. } = held.phase else {
                return;
            };
            if failed {
                held.phase = self.opened();
                self.entered(&held, Moved::Attempts { failed: Some(1) });
            } else if passed + 1 < self.probes {
                held.phase = Phase::BetweenProbes { passed: passed + 1 };
            } else {
                held.phase = Phase::Closed;
                held.forget();
                self.entered(&held, Moved::Attempts { failed: None });
            }
        } else if held.phase == Phase::Closed {
            let counted = held.count(self.window, failed, Instant::now);
            if counted >= self.failures {
                held.phase = self.opened();
                held.forget();
                self.entered(
                    &held,
                    Moved::Attempts {
                        failed: Some(counted),
                    },
                );
            }
        }
    }

    /// The probe's call postponed: it has answered by `until` at the latest.
    fn probe_postponed(&self, epoch: u64, until: Instant) {
        let Some(mut held) = self.held_since(epoch) else {
            return;
        };
        if let Phase::ProbePending { until: latest, .. } = &mut held.phase {
            *latest = until;
        }
    }

    /// The probe's call sent under `bound`: the breaker is half-open until it
    /// answers, which the first probe of a series files.
    fn probe_sent(&self, epoch: u64, bound: Duration) {
        let Some(mut held) = self.held_since(epoch) else {
            return;
        };
        if let Phase::ProbePending { passed, .. } = held.phase {
            held.phase = Phase::HalfOpen {
                until: Instant::now() + bound,
                passed,
            };
            if passed == 0 {
                self.entered(&held, Moved::Attempts { failed: None });
            }
        }
    }

    /// A probe given up before its call was sent: the next call probes
    /// instead, and the breaker, where it was all along, files no transition.
    fn withdraw_probe(&self, epoch: u64) {
        let Some(mut held) = self.held_since(epoch) else {
            return;
        };
        if let Phase::ProbePending { passed, .. } = held.phase {
            held.phase = match passed {
                0 => Phase::Open {
                    until: Instant::now(),
                },
                passed => Phase::BetweenProbes { passed },
            };
        }
    }

    /// A probe its caller dropped once its call was sent, before it answered
    /// and before its bound ended: the next call probes instead, the series
    /// keeping what it counted — and the breaker, half-open on no success yet,
    /// open again, saying so.
    fn abandon_probe(&self, epoch: u64) {
        let Some(mut held) = self.held_since(epoch) else {
            return;
        };
        if let Phase::HalfOpen { passed, .. } = held.phase {
            if passed > 0 {
                held.phase = Phase::BetweenProbes { passed };
                return;
            }
            held.phase = Phase::Open {
                until: Instant::now(),
            };
            self.entered(&held, Moved::Attempts { failed: None });
        }
    }

    fn opened(&self) -> Phase {
        Phase::Open {
            until: Instant::now() + self.open_for,
        }
    }

    /// Say the transition `held` just made, and count it: what failures opened
    /// it when they did, and that it was manual, with the open time it was
    /// given, when a manual switch moved it.
    ///
    /// Filed under the breaker's lock, which `held` borrows: no transition
    /// comes between this one and its line, so the lines say the transitions
    /// in the order they took effect, and the last one the state it is in.
    fn entered(&self, held: &Held, moved: Moved) {
        let (failed, open_for, manual) = match moved {
            Moved::Attempts { failed } => (failed, failed.map(|_| self.open_for), false),
            Moved::Manual { open_for } => (None, open_for, true),
        };
        let state = held.phase.as_str();
        tracing::warn!(
            target: TARGET,
            instance = self.instance,
            state,
            failed,
            window = failed.map(|_| self.window),
            open_ms = open_for.map(millis),
            manual = manual.then_some(true),
            "circuit breaker changed state",
        );
        self.counters.transition(state, manual);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

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
            None,
            1,
            Arc::new(Counters::new("upstream")),
        )
    }

    fn attempt(breaker: &Breaker, failed: bool) {
        breaker
            .admit(BOUND)
            .expect("a closed breaker admits")
            .send(BOUND, None)
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
        let probe = probe.send(BOUND, None);
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
        let probe = probe.send(3 * second, None);
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
        probe.send(BOUND, None).settle(true);
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
            .send(BOUND, None);
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
        let late = late.send(BOUND, None);
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
        probe.send(BOUND, None).settle(false);
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
        probe.send(BOUND, None).settle(true);
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
        probe.send(BOUND, None).settle(false);
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

    const AGE: Duration = Duration::from_secs(60);

    /// A breaker forgetting an outcome older than `age`, opening once
    /// `failures` of the latest `window` attempts failed.
    fn aged(window: usize, failures: usize, age: Duration) -> Breaker {
        Breaker::new(
            "upstream",
            window,
            failures,
            OPEN_FOR,
            Some(age),
            1,
            Arc::new(Counters::new("upstream")),
        )
    }

    /// A breaker one failure opens, and `probes` probes in a row close.
    fn probing(probes: usize) -> Breaker {
        Breaker::new(
            "upstream",
            1,
            1,
            OPEN_FOR,
            None,
            probes,
            Arc::new(Counters::new("upstream")),
        )
    }

    /// The next call, taking the probe's turn.
    fn probe(breaker: &Breaker) -> Pass<'_> {
        let pass = breaker.admit(BOUND).expect("the probe's turn is free");
        assert!(pass.probes(), "the call takes the probe's turn");
        pass
    }

    /// Each transition's line, as `state`, then `manual`, `open_ms` and
    /// `failed` where it carries them.
    fn said(lines: &nest_rs_testing::LogCapture) -> Vec<String> {
        lines
            .find(TARGET, "circuit breaker changed state")
            .iter()
            .map(|event| {
                assert_eq!(event.level, "warn", "{event:?}");
                assert_eq!(event.field("instance").as_deref(), Some("upstream"));
                let mut said = event.field("state").unwrap_or_default();
                if let Some(manual) = event.field("manual") {
                    assert_eq!(manual, "true", "{event:?}");
                    said.push_str(" manual");
                }
                for field in ["open_ms", "failed", "window"] {
                    if let Some(value) = event.field(field) {
                        said.push_str(&format!(" {field}={value}"));
                    }
                }
                said
            })
            .collect()
    }

    /// An open time under a millisecond, which `breaker_open` and a manual
    /// opening take in code above zero, is filed in whole milliseconds rounded
    /// up, `1`, never the `0` of no open time, whether failures or a hand
    /// opened the breaker for it.
    #[tokio::test(start_paused = true)]
    async fn an_open_time_under_a_millisecond_is_filed_as_one_never_as_zero() {
        let lines = nest_rs_testing::LogCapture::install();
        let under = Duration::from_micros(500);
        let breaker = Breaker::new(
            "upstream",
            1,
            1,
            under,
            None,
            1,
            Arc::new(Counters::new("upstream")),
        );
        attempt(&breaker, true);
        breaker.open_manually(under);
        assert_eq!(
            said(&lines),
            ["open open_ms=1 failed=1 window=1", "open manual open_ms=1"]
        );
    }

    /// An open time under a millisecond is read off the clock, never a timer:
    /// every call before its end is refused, told what is left of it, and the
    /// first at its end probes.
    #[tokio::test(start_paused = true)]
    async fn an_open_time_under_a_millisecond_refuses_until_its_end_and_probes_at_it() {
        let breaker = breaker(1, 1);
        let under = Duration::from_micros(500);
        breaker.open_manually(under);
        let tick = Duration::from_micros(1);
        tokio::time::advance(under - tick).await;
        assert_eq!(refused_for(&breaker), tick, "a microsecond before its end");
        tokio::time::advance(tick).await;
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A breaker one failure short of opening, idle past the age, does not
    /// open on the next failure: the window forgets its old outcomes as it
    /// counts the new one, with no call arriving in between.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_one_failure_short_and_idle_past_the_age_does_not_open_on_the_next_failure() {
        let breaker = aged(4, 2, AGE);
        attempt(&breaker, true);
        tokio::time::advance(AGE + Duration::from_millis(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure is past the age"
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open", "two failures inside the age");
    }

    /// The failing call that would open it is sent before the idle time and
    /// answers after it: its outcome, counted at its answer, finds the failure
    /// before it past the age.
    #[tokio::test(start_paused = true)]
    async fn a_failure_sent_before_the_idle_time_and_answering_after_it_opens_nothing() {
        let breaker = aged(4, 2, AGE);
        attempt(&breaker, true);
        let late = breaker.admit(BOUND).expect("closed").send(BOUND, None);
        tokio::time::advance(AGE + Duration::from_millis(1)).await;
        late.settle(true);
        assert_eq!(breaker.state(), "closed");
    }

    /// Every outcome past the age is forgotten as the next is counted, however
    /// many: a failure behind two successes as old leaves with them.
    #[tokio::test(start_paused = true)]
    async fn every_outcome_past_the_age_is_forgotten_as_the_next_is_counted() {
        let breaker = aged(10, 2, AGE);
        attempt(&breaker, false);
        attempt(&breaker, false);
        attempt(&breaker, true);
        tokio::time::advance(AGE + Duration::from_millis(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the three outcomes before it are past the age"
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open", "two failures inside the age");
    }

    /// A success past the age leaves the window taking no failure with it: a
    /// failure inside the age still counts.
    #[tokio::test(start_paused = true)]
    async fn a_success_past_the_age_takes_no_failure_with_it() {
        let breaker = aged(10, 2, AGE);
        attempt(&breaker, false);
        tokio::time::advance(AGE / 2).await;
        attempt(&breaker, true);
        tokio::time::advance(AGE / 2 + Duration::from_secs(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "the success is past the age, the failure after it 31 s old"
        );
    }

    /// An outcome counts until it is older than the age, the instant it is as
    /// old as the age included.
    #[tokio::test(start_paused = true)]
    async fn an_outcome_counts_until_it_is_older_than_the_age() {
        let breaker = aged(4, 2, AGE);
        attempt(&breaker, true);
        tokio::time::advance(AGE).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "the first failure is as old as the age"
        );
    }

    /// An age of no time, which the config refuses, forgets an outcome once the
    /// clock has moved past its answer: the window counts only the outcomes
    /// answered at one instant, as the age's floor says.
    #[tokio::test(start_paused = true)]
    async fn an_age_of_no_time_counts_only_the_outcomes_answered_at_one_instant() {
        let breaker = aged(4, 2, Duration::ZERO);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_millis(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure was answered before the clock moved"
        );
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "two failures answered at one instant"
        );
    }

    /// A breaker open for no time, which the config refuses, lets the next call
    /// through at once as its probe, every call beside it refused, as the open
    /// time's floor says.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_open_for_no_time_lets_the_next_call_through_at_once_as_its_probe() {
        let breaker = Breaker::new(
            "upstream",
            1,
            1,
            Duration::ZERO,
            None,
            1,
            Arc::new(Counters::new("upstream")),
        );
        attempt(&breaker, true);
        let _probe = probe(&breaker);
        assert_eq!(refused_for(&breaker), BOUND, "beside the probe");
    }

    /// Outcomes with an age still leave the window by its length.
    #[tokio::test(start_paused = true)]
    async fn an_aged_window_still_holds_its_latest_attempts_alone() {
        let breaker = aged(2, 2, AGE);
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

    /// A window holds its latest attempts and no more, with an age or without:
    /// of a failure, a success and a failure, a window of two holds the last
    /// two, one failure short of opening, where a window of three would open.
    #[tokio::test(start_paused = true)]
    async fn a_window_holds_its_latest_attempts_and_no_more_with_an_age_or_without() {
        for (breaker, kind) in [(breaker(2, 2), "no age"), (aged(2, 2, AGE), "an age")] {
            attempt(&breaker, true);
            attempt(&breaker, false);
            attempt(&breaker, true);
            assert_eq!(
                breaker.state(),
                "closed",
                "with {kind}: the first failure left"
            );
            attempt(&breaker, true);
            assert_eq!(breaker.state(), "open", "with {kind}");
        }
    }

    /// What a breaker counts once it forgot its window: one failure leaves it
    /// closed and a second opens it, no failure before counting.
    fn counts_afresh(breaker: &Breaker, forgot: &str) {
        assert_eq!(breaker.read(), BreakerState::Closed, "{forgot}");
        attempt(breaker, true);
        assert_eq!(breaker.state(), "closed", "{forgot}: one failure since");
        attempt(breaker, true);
        assert_eq!(breaker.state(), "open", "{forgot}: two failures since");
    }

    /// A breaker with an age forgets its window as one without does, whatever
    /// closes it: a manual closing, or the probe after a manual opening or
    /// after the failures that opened it.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_with_an_age_forgets_its_window_whatever_closes_it() {
        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        breaker.close_manually();
        counts_afresh(&breaker, "closed by hand");

        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        breaker.open_manually(OPEN_FOR);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        counts_afresh(&breaker, "opened by hand, closed by its probe");

        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        counts_afresh(&breaker, "opened by failures, closed by its probe");
    }

    /// With no age, the window holds a byte per outcome and no instant, and an
    /// outcome counts however old it is.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_with_no_age_keeps_no_instant_and_counts_an_outcome_however_old() {
        let breaker = breaker(4, 2);
        assert!(matches!(breaker.held.lock().outcomes, Outcomes::Counted(_)));
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(24 * 60 * 60)).await;
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    /// A window with no age counts an outcome without reading the clock it is
    /// handed, which only an age reads.
    #[test]
    fn a_window_with_no_age_counts_an_outcome_reading_no_clock() {
        let breaker = breaker(4, 2);
        let mut held = breaker.held.lock();
        let unread = || -> Instant { panic!("a window with no age reads no clock to count") };
        assert_eq!(held.count(4, true, unread), 1);
        assert_eq!(held.count(4, false, unread), 1);
        assert_eq!(held.count(4, true, unread), 2);
    }

    /// A breaker asking three probes closes on the third in a row, each let
    /// through alone, and its lines say it opened, probed and closed: nothing
    /// between two probes of the series, which changed no state.
    #[tokio::test(start_paused = true)]
    async fn a_series_of_probes_one_at_a_time_closes_the_breaker_on_its_last() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(3);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        for nth in 1..=3 {
            assert_eq!(breaker.read(), BreakerState::HalfOpen, "before probe {nth}");
            let pending = probe(&breaker);
            assert_eq!(refused_for(&breaker), BOUND, "probe {nth} holds the turn");
            let sent = pending.send(BOUND, None);
            assert_eq!(refused_for(&breaker), BOUND, "probe {nth}'s call is out");
            sent.settle(false);
        }
        assert_eq!(breaker.read(), BreakerState::Closed);
        attempt(&breaker, false);
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
    }

    /// A failed probe of a series opens the breaker again for its whole open
    /// time, and the series starts over.
    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_of_a_series_opens_the_breaker_again_and_the_series_starts_over() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(2);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        probe(&breaker).send(BOUND, None).settle(true);
        assert_eq!(refused_for(&breaker), OPEN_FOR);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "one probe of two since it opened again"
        );
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "closed"]
        );
    }

    /// A probe of a series dropped before it answers, its call sent or not,
    /// says nothing of the dependency: it counts as neither a success nor a
    /// failure, the series keeps its count, and nothing is filed.
    #[tokio::test(start_paused = true)]
    async fn a_probe_of_a_series_dropped_before_it_answers_counts_as_neither_and_keeps_the_count() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(3);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        drop(probe(&breaker));
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "dropped unsent");
        drop(probe(&breaker).send(BOUND, None));
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "dropped sent");
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "two of three");
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
    }

    /// An attempt sent under a bound ending at an instant and dropped
    /// unsettled at or past it has timed out: a failure in the window, a
    /// failed probe opening the breaker again for its whole open time. One
    /// dropped before that instant counts for nothing, its probe handed on.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_dropped_unsettled_at_its_bounds_end_has_timed_out() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(2);
        let until = || Some(Instant::now() + BOUND);
        drop(breaker.admit(BOUND).expect("closed").send(BOUND, until()));
        assert_eq!(breaker.read(), BreakerState::Closed, "dropped early");
        let sent = breaker.admit(BOUND).expect("closed").send(BOUND, until());
        tokio::time::advance(BOUND).await;
        drop(sent);
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            },
            "dropped at its bound's end, it failed"
        );

        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, until()));
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "a probe dropped early hands its turn on"
        );
        probe(&breaker).send(BOUND, None).settle(false);
        let sent = probe(&breaker).send(BOUND, until());
        tokio::time::advance(BOUND).await;
        drop(sent);
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            },
            "a probe dropped at its bound's end failed, one success of two in its series"
        );
        assert_eq!(
            said(&lines),
            [
                "open open_ms=30000 failed=1 window=1",
                "half_open",
                "open",
                "half_open",
                "open open_ms=30000 failed=1 window=1",
            ]
        );
    }

    /// The state read is what a call arriving now meets: closed, open with what
    /// is left of the open time, never zero, then half-open whether the
    /// probe's turn is free, taken by a call not sent yet — which the lines
    /// still say is open — or out.
    #[tokio::test(start_paused = true)]
    async fn the_state_read_is_what_a_call_arriving_now_meets() {
        let breaker = breaker(1, 1);
        assert_eq!(breaker.read(), BreakerState::Closed);
        attempt(&breaker, true);
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            }
        );
        tokio::time::advance(OPEN_FOR - Duration::from_millis(1)).await;
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: Duration::from_millis(1)
            }
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "the turn is free");
        let pending = probe(&breaker);
        assert_eq!(
            (breaker.read(), breaker.state(), refused_for(&breaker)),
            (BreakerState::HalfOpen, "open", BOUND),
            "the turn taken, its call not sent: no time read, the probe's bound told",
        );
        let sent = pending.send(BOUND, None);
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "the call out");
        sent.settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A manual opening refuses every call for its time, then lets one probe
    /// through as after any opening.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_refuses_every_call_for_its_time_then_probes() {
        let breaker = breaker(4, 2);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        assert_eq!(refused_for(&breaker), held_open);
        tokio::time::advance(held_open).await;
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(breaker.state(), "closed");
    }

    /// A manual opening of a breaker failures opened replaces its open time,
    /// counted from the call, shorter or longer: the switch does what it says.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_of_an_open_breaker_replaces_its_open_time_from_the_call() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(10)).await;
        breaker.open_manually(Duration::from_secs(5));
        assert_eq!(
            refused_for(&breaker),
            Duration::from_secs(5),
            "shorter than the 20 s left"
        );
        breaker.open_manually(Duration::from_secs(60));
        assert_eq!(refused_for(&breaker), Duration::from_secs(60), "longer");
    }

    /// A manual closing closes the breaker whatever it was in, and forgets its
    /// window: a failure counted before it adds to none after it.
    #[tokio::test(start_paused = true)]
    async fn a_manual_closing_closes_the_breaker_and_forgets_its_window() {
        let breaker = breaker(4, 2);
        attempt(&breaker, true);
        breaker.close_manually();
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the failure before it is forgotten"
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
        breaker.close_manually();
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A probe admitted or sent before a manual opening changes nothing when
    /// it answers a success, nor when it sends: the breaker stays open for the
    /// time it was given, and the probe after it, its own, alone closes it.
    #[tokio::test(start_paused = true)]
    async fn a_probe_admitted_or_sent_before_a_manual_opening_leaves_the_breaker_open_for_it() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let held_open = Duration::from_secs(10);

        let sent = probe(&breaker).send(BOUND, None);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker).send(BOUND, None);
        sent.settle(false);
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "the probe sent before the switch closed nothing"
        );
        current.settle(true);
        assert_eq!(refused_for(&breaker), OPEN_FOR, "the probe after it failed");

        tokio::time::advance(OPEN_FOR).await;
        let admitted = probe(&breaker);
        breaker.open_manually(held_open);
        assert!(!admitted.probes(), "the switch took its turn");
        tokio::time::advance(held_open).await;
        let current = probe(&breaker);
        admitted.send(BOUND, None).settle(false);
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "open", "open"],
            "the probe admitted before the switch filed no half_open when it sent"
        );
        assert!(
            current.probes(),
            "the probe admitted before the switch took nothing"
        );
        assert_eq!(breaker.read(), BreakerState::HalfOpen);
        current.send(BOUND, None).settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(
            states(&lines),
            [
                "open",
                "half_open",
                "open",
                "half_open",
                "open",
                "open",
                "half_open",
                "closed"
            ],
            "the probe after the switch filed its own half_open when it sent"
        );
    }

    /// A probe answering a failure after a manual closing leaves the breaker
    /// closed, and a probe of the breaker opened since as it was.
    #[tokio::test(start_paused = true)]
    async fn a_probe_answering_a_failure_after_a_manual_closing_leaves_the_breaker_closed() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let sent = probe(&breaker).send(BOUND, None);
        breaker.close_manually();
        sent.settle(true);
        assert_eq!(breaker.read(), BreakerState::Closed);

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let sent = probe(&breaker).send(BOUND, None);
        breaker.close_manually();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let current = probe(&breaker).send(BOUND, None);
        sent.settle(true);
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "the probe from before the switch opened nothing"
        );
        current.settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// An attempt admitted before a manual closing counts nothing in the window
    /// the switch forgot.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_admitted_before_a_manual_closing_counts_nothing_in_the_window_it_forgot() {
        let breaker = breaker(2, 2);
        attempt(&breaker, true);
        let late = breaker.admit(BOUND).expect("closed").send(BOUND, None);
        breaker.close_manually();
        late.settle(true);
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "closed", "one failure since the switch");
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    /// A probe dropped after a switch, its call sent or not, hands on no turn
    /// of the breaker's since: the probe holding it keeps it.
    #[tokio::test(start_paused = true)]
    async fn a_probe_dropped_after_a_switch_takes_nothing_from_the_probe_after_it() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let admitted = probe(&breaker);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker);
        drop(admitted);
        assert_eq!(refused_for(&breaker), BOUND, "the turn is still taken");
        let sent = current.send(BOUND, None);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let after = probe(&breaker).send(BOUND, None);
        drop(sent);
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "the call out is still the probe"
        );
        after.settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A probe admitted before a switch, postponing or sending its call once
    /// the probe after it took the turn, changes nothing of that turn: a call
    /// refused beside it is told the bound of the probe holding it, no
    /// `half_open` is filed, and the turn passes on when that probe is
    /// dropped unsent.
    #[tokio::test(start_paused = true)]
    async fn a_probe_postponed_or_sent_after_a_switch_takes_nothing_from_the_probe_after_it() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let stale = probe(&breaker);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker);
        stale.postpone(Instant::now() + 2 * BOUND, BOUND);
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "the bound of the probe holding the turn"
        );
        let stale = stale.send(BOUND, None);
        assert_eq!(
            breaker.state(),
            "open",
            "the probe holding the turn has not sent its call"
        );
        drop(current);
        stale.settle(false);
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(breaker.read(), BreakerState::Closed, "the turn passed on");
        assert_eq!(states(&lines), ["open", "open", "half_open", "closed"]);
    }

    /// A manual switch files the transition's `warn` and counts as one, marked
    /// manual — an opening with the time it was given — each time, whether the
    /// state it enters is the one it finds or not; no transition of the
    /// breaker's own carries the mark: failures opening it, a probe sent, a
    /// probe abandoned once sent, a failed probe, or the probe closing it.
    #[tokio::test(start_paused = true)]
    async fn a_manual_switch_is_the_transitions_warn_marked_manual() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        breaker.close_manually();
        breaker.open_manually(Duration::from_secs(45));
        breaker.open_manually(Duration::from_secs(45));
        breaker.close_manually();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, None));
        probe(&breaker).send(BOUND, None).settle(true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(
            said(&lines),
            [
                "closed manual",
                "open manual open_ms=45000",
                "open manual open_ms=45000",
                "closed manual",
                "open open_ms=30000 failed=1 window=1",
                "half_open",
                "open",
                "half_open",
                "open open_ms=30000 failed=1 window=1",
                "half_open",
                "closed",
            ]
        );
    }

    /// A manual switch counts by its instance and the state it entered, marked
    /// manual; no transition of the breaker's own carries the mark.
    #[cfg(feature = "opentelemetry")]
    #[tokio::test(start_paused = true)]
    async fn a_manual_switch_is_counted_by_the_state_entered_marked_manual() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        use crate::counters::{INSTANCE_ATTRIBUTE, MANUAL_ATTRIBUTE, STATE_ATTRIBUTE, TRANSITIONS};

        let breaker = breaker(1, 1);
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        breaker.open_manually(Duration::from_secs(10));
        breaker.close_manually();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, None));
        probe(&breaker).send(BOUND, None).settle(true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        provider.force_flush().expect("the export");

        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let mut counted: Vec<(String, String, u64)> = exported
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
                        assert_eq!(
                            attribute(INSTANCE_ATTRIBUTE),
                            "upstream",
                            "every point names its policy, a manual one too"
                        );
                        (
                            attribute(STATE_ATTRIBUTE),
                            attribute(MANUAL_ATTRIBUTE),
                            point.value(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        counted.sort();
        assert_eq!(
            counted,
            [
                ("closed".to_owned(), String::new(), 1),
                ("closed".to_owned(), "true".to_owned(), 1),
                ("half_open".to_owned(), String::new(), 3),
                ("open".to_owned(), String::new(), 3),
                ("open".to_owned(), "true".to_owned(), 1),
            ],
            "opened and closed by hand, then by the breaker: a failure, a probe abandoned, a \
             failed probe and the probe closing it"
        );
    }

    /// A caller on another thread at the instant a transition's line is filed:
    /// it switches the breaker by hand when the breaker's lock is free then —
    /// closing it, or opening it when the line says it closed — and the line
    /// is taken down after that switch's own.
    struct Racing {
        breaker: Arc<Breaker>,
        /// Set while the racing switch runs, whose own line races nothing.
        racing: AtomicBool,
        said: Arc<Mutex<Vec<String>>>,
    }

    /// A transition line's `state`, and whether it is marked `manual`.
    #[derive(Default)]
    struct Line {
        message: String,
        state: String,
        manual: bool,
    }

    impl tracing::field::Visit for Line {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "state" {
                value.clone_into(&mut self.state);
            }
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            if field.name() == "manual" {
                self.manual = value;
            }
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.message = format!("{value:?}");
            }
        }
    }

    impl tracing::Subscriber for Racing {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut line = Line::default();
            event.record(&mut line);
            if line.message != "circuit breaker changed state" {
                return;
            }
            if !self.racing.swap(true, Ordering::SeqCst) {
                if self.breaker.held.try_lock().is_some() {
                    if line.state == "closed" {
                        self.breaker.open_manually(OPEN_FOR);
                    } else {
                        self.breaker.close_manually();
                    }
                }
                self.racing.store(false, Ordering::SeqCst);
            }
            let manual = if line.manual { " manual" } else { "" };
            self.said.lock().push(format!("{}{manual}", line.state));
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// A transition files its line while it holds the breaker, so a manual
    /// switch racing it — from another thread, the instant the breaker is free
    /// — comes after that line, and the last line says the state the breaker
    /// is in, whatever moved it: failures, a probe sent, abandoned, failed or
    /// closing it, or another switch.
    #[tokio::test(start_paused = true)]
    async fn no_switch_comes_between_a_transition_and_its_line() {
        let breaker = Arc::new(breaker(1, 1));
        let said = Arc::new(Mutex::new(Vec::new()));
        let _racing = tracing::subscriber::set_default(Racing {
            breaker: Arc::clone(&breaker),
            racing: AtomicBool::new(false),
            said: Arc::clone(&said),
        });
        let last_said = || {
            let said = said.lock();
            let last = said.last().map_or("", String::as_str);
            last.split(' ').next().unwrap_or_default().to_owned()
        };
        attempt(&breaker, true);
        assert_eq!(last_said(), breaker.state(), "failures opened it");
        tokio::time::advance(OPEN_FOR).await;
        let sent = probe(&breaker).send(BOUND, None);
        assert_eq!(last_said(), breaker.state(), "a probe sent");
        drop(sent);
        assert_eq!(last_said(), breaker.state(), "the probe abandoned");
        probe(&breaker).send(BOUND, None).settle(true);
        assert_eq!(last_said(), breaker.state(), "a probe failed");
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(false);
        assert_eq!(last_said(), breaker.state(), "the probe closing it");
        breaker.open_manually(OPEN_FOR);
        assert_eq!(last_said(), breaker.state(), "opened by hand");
        breaker.close_manually();
        assert_eq!(last_said(), breaker.state(), "closed by hand");
        assert_eq!(
            *said.lock(),
            [
                "open",
                "half_open",
                "open",
                "half_open",
                "open",
                "half_open",
                "closed",
                "open manual",
                "closed manual",
            ],
            "no switch came between a transition and its line"
        );
    }
}
