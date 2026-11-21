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
