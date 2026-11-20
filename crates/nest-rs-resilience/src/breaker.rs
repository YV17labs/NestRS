//! [`CircuitBreaker`] — when a policy stops calling a failing dependency, and
//! how it finds out the dependency is back.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::TARGET;
use crate::stats::Counters;

/// A count-window circuit breaker.
///
/// **Closed**, it records the outcome of the last [`window`](Self::window)
/// attempts; once at least [`minimum_calls`](Self::minimum_calls) are recorded
/// and the share of failures reaches
/// [`failure_rate_percent`](Self::failure_rate_percent), it **opens**. Open,
/// it refuses every call for [`open_for`](Self::open_for), then turns
/// **half-open**: it lets [`half_open_probes`](Self::half_open_probes) calls
/// through, one at a time each, closing once that many succeed and opening
/// again on the first that fails. Every transition is a `warn`.
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
/// use nest_rs_resilience::CircuitBreaker;
///
/// let breaker = CircuitBreaker::new(
///     NonZeroU32::new(10).expect("non-zero"),
///     50,
///     Duration::from_secs(15),
/// )
/// .with_minimum_calls(5);
/// assert_eq!(breaker.minimum_calls(), 5);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CircuitBreaker {
    window: NonZeroU32,
    failure_rate_percent: u8,
    minimum_calls: u32,
    open_for: Duration,
    half_open_probes: NonZeroU32,
}

impl CircuitBreaker {
    /// Opens when half of the last 20 attempts failed, ten at least recorded;
    /// stays open 30 s, then closes on one successful probe.
    pub const DEFAULT: Self = Self::new(
        NonZeroU32::MIN.saturating_add(19),
        50,
        Duration::from_secs(30),
    )
    .with_minimum_calls(10);

    /// Opens when `failure_rate_percent` of the last `window` attempts failed
    /// (`window` of them at least recorded), and stays open `open_for`. A
    /// rate past 100 is held to 100, and a rate of 0 to 1.
    pub const fn new(window: NonZeroU32, failure_rate_percent: u8, open_for: Duration) -> Self {
        let failure_rate_percent = if failure_rate_percent > 100 {
            100
        } else if failure_rate_percent == 0 {
            1
        } else {
            failure_rate_percent
        };
        Self {
            window,
            failure_rate_percent,
            minimum_calls: window.get(),
            open_for,
            half_open_probes: NonZeroU32::MIN,
        }
    }

    /// Judges the window once `calls` attempts are recorded, held to
    /// `1..=window`.
    pub const fn with_minimum_calls(mut self, calls: u32) -> Self {
        self.minimum_calls = if calls == 0 {
            1
        } else if calls > self.window.get() {
            self.window.get()
        } else {
            calls
        };
        self
    }

    /// Closes once `probes` half-open calls succeed.
    pub const fn with_half_open_probes(mut self, probes: NonZeroU32) -> Self {
        self.half_open_probes = probes;
        self
    }

    /// How many of the last attempts the breaker judges.
    pub const fn window(&self) -> NonZeroU32 {
        self.window
    }

    /// The share of failures, in percent, that opens the breaker.
    pub const fn failure_rate_percent(&self) -> u8 {
        self.failure_rate_percent
    }

    /// How many attempts must be recorded before the window is judged.
    pub const fn minimum_calls(&self) -> u32 {
        self.minimum_calls
    }

    /// How long the breaker stays open before letting a probe through.
    pub const fn open_for(&self) -> Duration {
        self.open_for
    }

    /// How many half-open probes must succeed to close the breaker.
    pub const fn half_open_probes(&self) -> NonZeroU32 {
        self.half_open_probes
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Where a breaker stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CircuitState {
    /// Calls go through, and their outcomes are recorded.
    Closed,
    /// Calls are refused until the open window elapses.
    Open,
    /// Probes go through, one at a time each, deciding whether to close.
    HalfOpen,
}

#[derive(Debug)]
enum Phase {
    Closed,
    Open { until: Instant },
    HalfOpen { in_flight: u32, succeeded: u32 },
}

impl Phase {
    const fn state(&self) -> CircuitState {
        match self {
            Self::Closed => CircuitState::Closed,
            Self::Open { .. } => CircuitState::Open,
            Self::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }
}

#[derive(Debug)]
struct Window {
    phase: Phase,
    /// The last outcomes, `true` for a failure.
    outcomes: VecDeque<bool>,
    failures: u32,
}

/// One breaker's live state, shared by every caller of its policy.
#[derive(Debug)]
pub(crate) struct Breaker {
    spec: CircuitBreaker,
    policy: Arc<str>,
    window: Mutex<Window>,
}

/// What [`Breaker::admit`] let through: an ordinary call, or a half-open probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    Closed,
    Probe,
}

/// One admitted attempt, which records its outcome once. Dropped unrecorded —
/// the caller's future was dropped mid-attempt — a probe gives its slot back
/// and counts neither way.
pub(crate) struct Ticket<'a> {
    breaker: &'a Breaker,
    admission: Admission,
    recorded: bool,
}

impl Ticket<'_> {
    pub(crate) fn record(mut self, failed: bool, counters: &Counters) {
        self.recorded = true;
        self.breaker
            .record(self.admission, failed, Instant::now(), counters);
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if !self.recorded && self.admission == Admission::Probe {
            let mut window = self.breaker.window.lock();
            if let Phase::HalfOpen { in_flight, .. } = &mut window.phase {
                *in_flight = in_flight.saturating_sub(1);
            }
        }
    }
}

impl Breaker {
    pub(crate) fn new(spec: CircuitBreaker, policy: Arc<str>) -> Self {
        Self {
            spec,
            policy,
            window: Mutex::new(Window {
                phase: Phase::Closed,
                outcomes: VecDeque::with_capacity(spec.window.get() as usize),
                failures: 0,
            }),
        }
    }

    pub(crate) fn state(&self) -> CircuitState {
        let now = Instant::now();
        let window = self.window.lock();
        match window.phase {
            // An elapsed open window is half-open to the next caller.
            Phase::Open { until } if now >= until => CircuitState::HalfOpen,
            ref phase => phase.state(),
        }
    }

    /// Lets an attempt through, or answers how long until the breaker would.
    pub(crate) fn admit(&self) -> Result<Ticket<'_>, Duration> {
        let now = Instant::now();
        let mut window = self.window.lock();
        if let Phase::Open { until } = window.phase {
            if now < until {
                return Err(until - now);
            }
            self.transition(
                &mut window,
                Phase::HalfOpen {
                    in_flight: 0,
                    succeeded: 0,
                },
            );
        }
        let probes = self.spec.half_open_probes.get();
        let admission = match &mut window.phase {
            Phase::Closed => Admission::Closed,
            Phase::HalfOpen {
                in_flight,
                succeeded,
            } => {
                if in_flight.saturating_add(*succeeded) >= probes {
                    // Every probe the breaker needs is out: the others wait on
                    // their verdict, about a probe's round trip.
                    return Err(self.spec.open_for.min(Duration::from_secs(1)));
                }
                *in_flight += 1;
                Admission::Probe
            }
            Phase::Open { until } => return Err(until.saturating_duration_since(now)),
        };
        Ok(Ticket {
            breaker: self,
            admission,
            recorded: false,
        })
    }

    fn record(&self, admission: Admission, failed: bool, now: Instant, counters: &Counters) {
        let mut window = self.window.lock();
        match (admission, &mut window.phase) {
            (
                Admission::Probe,
                Phase::HalfOpen {
                    in_flight,
                    succeeded,
                },
            ) => {
                *in_flight = in_flight.saturating_sub(1);
                if failed {
                    Counters::bump(&counters.opened);
                    self.transition(
                        &mut window,
                        Phase::Open {
                            until: now + self.spec.open_for,
                        },
                    );
                } else {
                    *succeeded += 1;
                    if *succeeded >= self.spec.half_open_probes.get() {
                        self.transition(&mut window, Phase::Closed);
                    }
                }
            }
            (Admission::Closed, Phase::Closed) => {
                let capacity = self.spec.window.get() as usize;
                if window.outcomes.len() >= capacity
                    && let Some(oldest) = window.outcomes.pop_front()
                    && oldest
                {
                    window.failures -= 1;
                }
                window.outcomes.push_back(failed);
                if failed {
                    window.failures += 1;
                }
                let recorded = u32::try_from(window.outcomes.len()).unwrap_or(u32::MAX);
                let tripped = recorded >= self.spec.minimum_calls
                    && u64::from(window.failures) * 100
                        >= u64::from(self.spec.failure_rate_percent) * u64::from(recorded);
                if tripped {
                    Counters::bump(&counters.opened);
                    self.transition(
                        &mut window,
                        Phase::Open {
                            until: now + self.spec.open_for,
                        },
                    );
                }
            }
            // The phase moved while the attempt ran — another caller opened
            // the breaker, or a probe closed it: this outcome belongs to a
            // phase that is over.
            _ => {}
        }
    }

    fn transition(&self, window: &mut Window, to: Phase) {
        let from = window.phase.state();
        let state = to.state();
        window.phase = to;
        if state != CircuitState::HalfOpen {
            window.outcomes.clear();
            window.failures = 0;
        }
        tracing::warn!(
            target: TARGET,
            policy = %self.policy,
            from = ?from,
            to = ?state,
            "circuit breaker transition",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaker(window: u32, rate: u8, min: u32) -> Breaker {
        Breaker::new(
            CircuitBreaker::new(
                NonZeroU32::new(window).unwrap(),
                rate,
                Duration::from_secs(10),
            )
            .with_minimum_calls(min),
            Arc::from("test"),
        )
    }

    fn outcome(breaker: &Breaker, failed: bool, counters: &Counters) {
        breaker.admit().unwrap().record(failed, counters);
    }

    #[test]
    fn spec_holds_its_values_to_their_ranges() {
        let spec = CircuitBreaker::new(NonZeroU32::new(5).unwrap(), 200, Duration::ZERO)
            .with_minimum_calls(50);
        assert_eq!(spec.failure_rate_percent(), 100);
        assert_eq!(spec.minimum_calls(), 5);
        let spec = CircuitBreaker::new(NonZeroU32::new(5).unwrap(), 0, Duration::ZERO)
            .with_minimum_calls(0);
        assert_eq!(spec.failure_rate_percent(), 1);
        assert_eq!(spec.minimum_calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn opens_once_the_window_reaches_the_rate() {
        let counters = Counters::default();
        let breaker = breaker(4, 50, 4);
        outcome(&breaker, true, &counters);
        outcome(&breaker, false, &counters);
        outcome(&breaker, false, &counters);
        assert_eq!(breaker.state(), CircuitState::Closed);
        outcome(&breaker, true, &counters);
        assert_eq!(breaker.state(), CircuitState::Open);
        assert_eq!(counters.snapshot().opened, 1);
        let wait = breaker.admit().err().unwrap();
        assert_eq!(wait, Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn the_window_slides() {
        let counters = Counters::default();
        let breaker = breaker(3, 100, 3);
        outcome(&breaker, true, &counters);
        outcome(&breaker, true, &counters);
        outcome(&breaker, false, &counters);
        // The oldest outcomes slide out; two of three is under 100 %.
        outcome(&breaker, true, &counters);
        outcome(&breaker, true, &counters);
        assert_eq!(breaker.state(), CircuitState::Closed);
        // The success slides out too: three of three.
        outcome(&breaker, true, &counters);
        assert_eq!(breaker.state(), CircuitState::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_lets_one_probe_through_and_closes_on_its_success() {
        let counters = Counters::default();
        let breaker = breaker(1, 100, 1);
        outcome(&breaker, true, &counters);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        let probe = breaker.admit().unwrap();
        assert!(breaker.admit().is_err(), "one probe at a time");
        probe.record(false, &counters);
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_opens_again() {
        let counters = Counters::default();
        let breaker = breaker(1, 100, 1);
        outcome(&breaker, true, &counters);
        tokio::time::advance(Duration::from_secs(10)).await;
        outcome(&breaker, true, &counters);
        assert_eq!(breaker.state(), CircuitState::Open);
        assert_eq!(counters.snapshot().opened, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_probe_gives_its_slot_back() {
        let counters = Counters::default();
        let breaker = breaker(1, 100, 1);
        outcome(&breaker, true, &counters);
        tokio::time::advance(Duration::from_secs(10)).await;
        drop(breaker.admit().unwrap());
        outcome(&breaker, false, &counters);
        assert_eq!(breaker.state(), CircuitState::Closed);
    }
}
