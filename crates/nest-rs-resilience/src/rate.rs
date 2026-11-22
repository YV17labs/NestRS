//! The outbound rate a policy keeps: how many attempts it sends its dependency
//! per window, a fixed window the first attempt counted in it opens, as the
//! throttler's stores count a client's. Its count lives in the process.

use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

/// A bound on the attempts one policy sends per window.
pub(crate) struct Rate {
    limit: usize,
    window: Duration,
    /// How long one attempt waits in all for the window's reset.
    wait: Duration,
    /// The window counting, once an attempt opened one.
    held: Mutex<Option<Window>>,
}

/// One window: when its first attempt was counted, and how many were since.
struct Window {
    opened: Instant,
    counted: usize,
}

impl Rate {
    /// A rate sending `limit` attempts per `window`, each attempt past it
    /// waiting up to `wait` in all for the window's reset.
    pub(crate) fn new(limit: usize, window: Duration, wait: Duration) -> Self {
        Self {
            limit,
            window,
            wait,
            held: Mutex::new(None),
        }
    }

    /// Count an attempt sent at `now` — opening a window when none is counting
    /// — or refuse it for the time left to the window's reset, counting
    /// nothing: a refused attempt is never sent.
    pub(crate) fn count(&self, now: Instant) -> Result<(), Duration> {
        let mut held = self.held.lock();
        if let Some(window) = held.as_mut() {
            let elapsed = now.saturating_duration_since(window.opened);
            if elapsed < self.window {
                if window.counted >= self.limit {
                    return Err(self.window - elapsed);
                }
                window.counted += 1;
                return Ok(());
            }
        }
        *held = Some(Window {
            opened: now,
            counted: 1,
        });
        Ok(())
    }

    /// How long one attempt waits in all for the window's reset.
    pub(crate) const fn wait(&self) -> Duration {
        self.wait
    }

    /// The most attempts the rate sends within any span `span` long, both
    /// ends included: each window opening inside it sends its limit, and one
    /// open as it starts all but its first attempt, sent before it. That one
    /// pushes every later opening past the span's start, so a span a whole
    /// number of windows long holds the most with none open, a window opening
    /// at each of its ends.
    pub(crate) fn most_sent_within(&self, span: Duration) -> usize {
        let (span, window) = (span.as_nanos(), self.window.as_nanos());
        let Some(whole) = span
            .checked_div(window)
            .and_then(|whole| usize::try_from(whole).ok())
        else {
            return usize::MAX;
        };
        if span.is_multiple_of(window) {
            self.limit.saturating_mul(whole.saturating_add(1))
        } else {
            self.limit
                .saturating_mul(whole.saturating_add(2))
                .saturating_sub(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(10);

    fn counted(rate: &Rate) -> Result<(), Duration> {
        rate.count(Instant::now())
    }

    /// The window opens at its first attempt, not on the clock's beat, counts
    /// every attempt until it has passed, and the next attempt opens the next.
    #[tokio::test(start_paused = true)]
    async fn the_first_attempt_counted_opens_a_fixed_window_the_rest_count_in() {
        let rate = Rate::new(2, WINDOW, Duration::ZERO);
        tokio::time::advance(Duration::from_secs(3)).await;
        assert_eq!(counted(&rate), Ok(()), "opens the window, at 3 s");
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(counted(&rate), Ok(()));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            counted(&rate),
            Err(Duration::from_secs(7)),
            "the third in the window, told its reset at 13 s"
        );
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(counted(&rate), Ok(()), "opens the next window, at 13 s");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            counted(&rate),
            Ok(()),
            "a window sliding over the attempt at 5 s would refuse this one"
        );
        assert_eq!(counted(&rate), Err(Duration::from_secs(9)));
    }

    /// However many attempts a window refused, the next one opens with every
    /// attempt its limit allows: a refusal carries no debt over.
    #[tokio::test(start_paused = true)]
    async fn the_attempts_a_window_refused_take_nothing_of_the_next() {
        let rate = Rate::new(2, WINDOW, Duration::ZERO);
        assert_eq!(counted(&rate), Ok(()));
        assert_eq!(counted(&rate), Ok(()));
        for _ in 0..5 {
            assert_eq!(counted(&rate), Err(WINDOW));
        }
        tokio::time::advance(WINDOW).await;
        assert_eq!((counted(&rate), counted(&rate)), (Ok(()), Ok(())));
        assert_eq!(counted(&rate), Err(WINDOW));
    }

    /// How many attempts `rate` sends within `span`, both ends included, each
    /// sent as soon as the rate lets it through, from a start with no window
    /// open, or with one its first attempt opened `before` it.
    fn sent_within(rate: &Rate, span: Duration, before: Option<Duration>) -> usize {
        let starts = Instant::now() + WINDOW;
        if let Some(before) = before {
            assert_eq!(rate.count(starts - before), Ok(()), "opens a window");
        }
        let ends = starts + span;
        let (mut now, mut sent) = (starts, 0);
        loop {
            match rate.count(now) {
                Ok(()) => sent += 1,
                Err(reset) if now + reset <= ends => now += reset,
                Err(_) => return sent,
            }
        }
    }

    /// The most attempts a span holds are the most the rate sends in it from
    /// any start: all but the first of a window open as it starts, and a full
    /// window as each later one opens inside it. That window pushes every
    /// later opening past the span's start, so a span a whole number of
    /// windows long holds the most with none open, a window opening at each of
    /// its ends: two per 10 s send three within 5 s, six within 20 s, and
    /// seven within 25 s.
    #[test]
    fn the_most_attempts_a_span_holds_are_the_most_the_rate_sends_in_it_from_any_start() {
        let tick = Duration::from_nanos(1);
        let rate = |limit| Rate::new(limit, WINDOW, Duration::ZERO);
        let most_sent = |limit, span| {
            [None, Some(tick), Some(WINDOW / 2), Some(WINDOW - tick)]
                .map(|before| sent_within(&rate(limit), span, before))
                .into_iter()
                .max()
        };
        assert_eq!(most_sent(2, Duration::from_secs(5)), Some(3));
        assert_eq!(most_sent(2, Duration::from_secs(20)), Some(6));
        assert_eq!(most_sent(2, Duration::from_secs(25)), Some(7));
        for limit in 1..=3 {
            for whole in 0..=3 {
                for span in [
                    WINDOW * whole,
                    WINDOW * whole + tick,
                    WINDOW * whole + WINDOW / 2,
                    WINDOW * (whole + 1) - tick,
                ] {
                    assert_eq!(
                        most_sent(limit, span),
                        Some(rate(limit).most_sent_within(span)),
                        "{limit} per {WINDOW:?} within {span:?}"
                    );
                }
            }
        }
    }

    /// An attempt read before another opened the window counts in that one,
    /// its earlier clock read as no time elapsed.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_read_before_the_window_opened_counts_in_it() {
        let rate = Rate::new(1, WINDOW, Duration::ZERO);
        let earlier = Instant::now();
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(counted(&rate), Ok(()));
        assert_eq!(rate.count(earlier), Err(WINDOW));
    }
}
