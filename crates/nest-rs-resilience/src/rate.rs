//! The outbound rate a policy keeps: a token bucket holding `limit` tokens,
//! one more every `window / limit` until it is full, an attempt taking one to
//! be sent or, none being free, waiting in line for the next. It is kept in
//! whole numbers, as the generic cell rate algorithm keeps one (ITU-T I.371's
//! virtual scheduling): the bucket is the one instant it is full again at.
//! Its tokens live in the process.

use std::collections::VecDeque;
use std::task::Waker;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

/// A bound on the attempts one policy sends: `limit` at once from a bucket
/// left to fill, then `limit` per `window`.
pub(crate) struct Rate {
    limit: u128,
    /// The time between two tokens, in ticks. A tick is a nanosecond's
    /// `limit`th, so this is the window's nanoseconds whatever the limit: no
    /// interval is rounded, one under a nanosecond included.
    between: u128,
    /// How long before it is full the bucket has a token, in ticks: the time
    /// `limit - 1` tokens take.
    depth: u128,
    /// The longest an attempt waits for its token, `None` for no wait.
    wait: Option<Duration>,
    /// Where ticks count from, the bucket full.
    origin: Instant,
    held: Mutex<Bucket>,
}

struct Bucket {
    /// The tick the bucket is full again at, counting every token taken.
    full_at: u128,
    /// The attempts waiting for a token, in arrival order: the one with `n`
    /// before it is due the token after the `n` next.
    line: VecDeque<Waiting>,
    /// How many attempts ever waited: the next one's place.
    arrived: u64,
}

/// One attempt in the line.
struct Waiting {
    place: u64,
    /// Wakes it when a reservation before it is given back, its token due
    /// sooner than the timer it armed.
    waker: Option<Waker>,
}

/// What an attempt arriving for a token is given.
pub(crate) enum Arrived<'r> {
    /// A token: the attempt is sent.
    Taken,
    /// The next token no attempt before it waits for, free at the instant.
    Reserved(Reservation<'r>, Instant),
    /// No token it could wait for, and nothing of the bucket: the time to the
    /// next one free.
    Refused(Duration),
}

/// An attempt's place in the line for a token, given back when dropped: every
/// attempt behind it is then due its token one sooner, and woken to hear it.
pub(crate) struct Reservation<'r> {
    rate: &'r Rate,
    place: u64,
}

impl Rate {
    /// A bucket of `limit` tokens, full, refilled over `window`, an attempt
    /// finding none free waiting for the next only when it comes within
    /// `wait`, refused at once otherwise or with no `wait`.
    pub(crate) fn new(limit: usize, window: Duration, wait: Option<Duration>) -> Self {
        // `ResilienceConfig` takes no limit of none; one would divide by it.
        let limit = limit.max(1) as u128;
        let between = window.as_nanos();
        Self {
            limit,
            between,
            depth: between.saturating_mul(limit - 1),
            wait,
            origin: Instant::now(),
            held: Mutex::new(Bucket {
                full_at: 0,
                line: VecDeque::new(),
                arrived: 0,
            }),
        }
    }

    /// Take a token for an attempt arriving at `now`, behind every attempt
    /// waiting for one. With none free it reserves the next, when that one
    /// comes within the rate's wait and before the unit's `deadline`, at
    /// which a call sent has no time to answer; otherwise it is refused, told
    /// the time to that token, and takes nothing.
    pub(crate) fn arrive(&self, now: Instant, deadline: Option<Instant>) -> Arrived<'_> {
        let mut held = self.held.lock();
        let ahead = held.line.len();
        let due = self.due(&held, ahead);
        if self.tick(now) >= due {
            self.take(&mut held, now);
            return Arrived::Taken;
        }
        let at = self.instant(due);
        if !self.waits(now, at, deadline) {
            return Arrived::Refused(at.saturating_duration_since(now));
        }
        let place = held.arrived;
        held.arrived += 1;
        held.line.push_back(Waiting { place, waker: None });
        Arrived::Reserved(Reservation { rate: self, place }, at)
    }

    /// How long a call at `now` is refused for, the time to the token it is due
    /// behind the line as it stands, when no wait it may make reaches that
    /// token: it arrives for it `within` this long at the latest — its wait
    /// for a slot of the bulkhead — and waits the rate's own wait from there,
    /// and the token lies past both, or at the unit's `deadline` or past it.
    /// With `within` none it is what [`arrive`](Self::arrive) would answer.
    /// It takes and reserves nothing, so the call it lets wait meets the rate
    /// as it stands once it arrives.
    pub(crate) fn refusing(
        &self,
        now: Instant,
        within: Duration,
        deadline: Option<Instant>,
    ) -> Option<Duration> {
        let held = self.held.lock();
        let due = self.due(&held, held.line.len());
        if self.tick(now) >= due {
            return None;
        }
        let at = self.instant(due);
        let away = at.saturating_duration_since(now);
        let reached = within.saturating_add(self.wait.unwrap_or_default());
        (away > reached || deadline.is_some_and(|deadline| at >= deadline)).then_some(away)
    }

    /// The whole tokens free at `now`: what as many attempts arriving
    /// together would take, none while an attempt waits for one.
    pub(crate) fn available(&self, now: Instant) -> usize {
        let held = self.held.lock();
        // Not read off `due`, which stops at the origin: a bucket taken from
        // there holds tokens due before it.
        let taken = held
            .full_at
            .saturating_add(self.between.saturating_mul(held.line.len() as u128));
        let Some(since) = self.tick(now).saturating_add(self.depth).checked_sub(taken) else {
            return 0;
        };
        let free = since
            .checked_div(self.between)
            .map_or(self.limit, |refilled| self.limit.min(refilled + 1));
        usize::try_from(free).unwrap_or(usize::MAX)
    }

    /// The most attempts the bucket lets out within any span `span` long,
    /// both ends included: a full bucket as it starts, then each token that
    /// comes before it ends.
    pub(crate) fn most_sent_within(&self, span: Duration) -> usize {
        span.as_nanos()
            .saturating_mul(self.limit)
            .checked_div(self.between)
            .and_then(|refilled| usize::try_from(self.limit.saturating_add(refilled)).ok())
            .unwrap_or(usize::MAX)
    }

    /// Whether an attempt at `now` waits for a token free at `at`.
    fn waits(&self, now: Instant, at: Instant, deadline: Option<Instant>) -> bool {
        self.wait
            .is_some_and(|wait| at.saturating_duration_since(now) <= wait)
            && deadline.is_none_or(|deadline| at < deadline)
    }

    /// `at` in ticks, none before the origin.
    fn tick(&self, at: Instant) -> u128 {
        at.saturating_duration_since(self.origin)
            .as_nanos()
            .saturating_mul(self.limit)
    }

    /// The first instant at or past `tick`: a wait told is never short of its
    /// token, and one above none is a nanosecond at least.
    fn instant(&self, tick: u128) -> Instant {
        self.origin + Duration::from_nanos_u128(tick.div_ceil(self.limit))
    }

    /// The tick an attempt with `ahead` attempts before it is due its token at.
    fn due(&self, held: &Bucket, ahead: usize) -> u128 {
        held.full_at
            .saturating_add(self.between.saturating_mul(ahead as u128))
            .saturating_sub(self.depth)
    }

    /// One token taken at `now`, by an attempt arriving or by one of the line,
    /// still in it.
    fn take(&self, held: &mut Bucket, now: Instant) {
        // A full bucket gains nothing, the tokens its line is due set aside:
        // however late an attempt takes its own, the ones behind it stay due.
        let set_aside = self.between.saturating_mul(held.line.len() as u128);
        let full = self.tick(now).saturating_sub(set_aside);
        held.full_at = held.full_at.max(full).saturating_add(self.between);
    }
}

impl Reservation<'_> {
    /// The instant the token is free at, as the line stands, `waker` woken
    /// when a reservation given back brings it forward.
    pub(crate) fn due(&self, waker: &Waker) -> Instant {
        let mut held = self.rate.held.lock();
        let ahead = self.ahead(&held);
        let armed = &mut held.line[ahead].waker;
        let stale = match armed {
            Some(armed) if armed.will_wake(waker) => None,
            _ => armed.replace(waker.clone()),
        };
        let at = self.rate.instant(self.rate.due(&held, ahead));
        // A waker is dropped, as it is woken, once the lock is let go: either
        // runs its runtime's code.
        drop(held);
        drop(stale);
        at
    }

    /// Take the token at `now` and leave the line, or keep the place, told
    /// the instant the token is free at, when `now` is before it.
    pub(crate) fn take(self, now: Instant) -> Result<(), (Self, Instant)> {
        let mut held = self.rate.held.lock();
        let ahead = self.ahead(&held);
        let due = self.rate.due(&held, ahead);
        if self.rate.tick(now) < due {
            let at = self.rate.instant(due);
            drop(held);
            return Err((self, at));
        }
        self.rate.take(&mut held, now);
        let left = held.line.remove(ahead);
        drop(held);
        drop(left);
        std::mem::forget(self);
        Ok(())
    }

    /// How many attempts wait before this one.
    fn ahead(&self, held: &Bucket) -> usize {
        held.line
            .binary_search_by_key(&self.place, |waiting| waiting.place)
            .unwrap_or_else(|_| unreachable!("a reservation holds its place until it is dropped"))
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut held = self.rate.held.lock();
        let ahead = self.ahead(&held);
        let left = held.line.remove(ahead);
        let behind: Vec<Waker> = held
            .line
            .range_mut(ahead..)
            .filter_map(|waiting| waiting.waker.take())
            .collect();
        drop(held);
        drop(left);
        behind.into_iter().for_each(Waker::wake);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    use super::*;

    const WINDOW: Duration = Duration::from_secs(10);
    const SECOND: Duration = Duration::from_secs(1);
    const NANO: Duration = Duration::from_nanos(1);
    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// What `rate` gives an attempt arriving at `at` under no deadline, said in
    /// a word: the wait it reserved its token for is given back at once.
    fn arriving(rate: &Rate, at: Instant) -> Result<(), Duration> {
        match rate.arrive(at, None) {
            Arrived::Taken => Ok(()),
            Arrived::Reserved(_given_back, due) => Err(due - at),
            Arrived::Refused(wait) => Err(wait),
        }
    }

    /// The reservation `rate` gives an attempt arriving at `at`, and the
    /// instant its token is free at.
    fn reserved(rate: &Rate, at: Instant) -> (Reservation<'_>, Instant) {
        match rate.arrive(at, None) {
            Arrived::Reserved(reservation, due) => (reservation, due),
            Arrived::Taken => panic!("a token was free"),
            Arrived::Refused(wait) => panic!("refused for {wait:?}"),
        }
    }

    /// Counts how often it is woken.
    #[derive(Default)]
    struct Woken(AtomicUsize);

    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl Woken {
        fn times(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }

    /// The bucket starts full, with no window to open: its limit goes out at
    /// once, the next attempt is told the time to the first token back, and a
    /// token comes every `window / limit` from the first one taken, whenever
    /// the others were.
    #[tokio::test(start_paused = true)]
    async fn the_bucket_starts_full_and_a_token_comes_back_every_limith_of_the_window() {
        let rate = Rate::new(2, WINDOW, None);
        let origin = Instant::now();
        let at = |secs| origin + SECOND * secs;
        assert_eq!(rate.available(at(3)), 2, "full, however long it was left");
        assert_eq!(arriving(&rate, at(3)), Ok(()));
        assert_eq!(arriving(&rate, at(5)), Ok(()));
        assert_eq!(
            arriving(&rate, at(6)),
            Err(SECOND * 2),
            "the first token is back 5 s after it was taken, at 8 s"
        );
        assert_eq!(arriving(&rate, at(8)), Ok(()), "at 8 s");
        assert_eq!(
            arriving(&rate, at(9)),
            Err(SECOND * 4),
            "the next 5 s later, at 13 s: a fixed window opened at 3 s would send two more there"
        );
        assert_eq!(arriving(&rate, at(13)), Ok(()));
        assert_eq!(arriving(&rate, at(13)), Err(SECOND * 5));
    }

    /// Left alone the bucket fills up to its limit and no further: after any
    /// idle spell it sends its limit at once, then one token per interval.
    #[tokio::test(start_paused = true)]
    async fn the_bucket_refills_continuously_and_never_past_its_limit() {
        let rate = Rate::new(4, WINDOW, None);
        let origin = Instant::now();
        let interval = WINDOW / 4;
        for _ in 0..4 {
            assert_eq!(arriving(&rate, origin), Ok(()));
        }
        for (after, free) in [
            (Duration::ZERO, 0),
            (interval - NANO, 0),
            (interval, 1),
            (interval * 2 - NANO, 1),
            (interval * 2, 2),
            (WINDOW - NANO, 3),
            (WINDOW, 4),
            (WINDOW + interval, 4),
            (WINDOW * 100, 4),
        ] {
            assert_eq!(rate.available(origin + after), free, "{after:?} on");
        }
        let later = origin + WINDOW * 100;
        for _ in 0..4 {
            assert_eq!(arriving(&rate, later), Ok(()));
        }
        assert_eq!(
            arriving(&rate, later),
            Err(interval),
            "a hundred windows idle held four tokens, not four hundred"
        );
    }

    /// However many attempts the bucket refused, the next token goes to the
    /// next attempt at the instant every one of them was told: a refusal
    /// reserves nothing and carries no debt over.
    #[tokio::test(start_paused = true)]
    async fn a_refused_attempt_takes_nothing() {
        let rate = Rate::new(2, WINDOW, None);
        let origin = Instant::now();
        assert_eq!(arriving(&rate, origin), Ok(()));
        assert_eq!(arriving(&rate, origin), Ok(()));
        for _ in 0..5 {
            assert_eq!(arriving(&rate, origin), Err(WINDOW / 2));
            assert_eq!(
                rate.refusing(origin, Duration::ZERO, None),
                Some(WINDOW / 2)
            );
        }
        assert_eq!(rate.available(origin + WINDOW / 2), 1);
        assert_eq!(arriving(&rate, origin + WINDOW / 2), Ok(()));
        assert_eq!(arriving(&rate, origin + WINDOW / 2), Err(WINDOW / 2));
    }

    /// A read says what arriving would answer, at every point, and takes
    /// nothing: however often the bucket is read, it lets as many attempts
    /// through as before. With a wait that reaches the next token, an attempt
    /// arriving would wait rather than be refused, and the read says so.
    #[tokio::test(start_paused = true)]
    async fn a_read_of_the_rate_answers_what_arriving_would_and_takes_nothing() {
        let rate = Rate::new(2, WINDOW, None);
        let origin = Instant::now();
        for _ in 0..2 {
            for _ in 0..5 {
                assert_eq!(
                    rate.refusing(origin, Duration::ZERO, None),
                    None,
                    "a token free"
                );
            }
            assert_eq!(arriving(&rate, origin), Ok(()));
        }
        assert_eq!(
            rate.refusing(origin, Duration::ZERO, None),
            Some(SECOND * 5),
            "empty"
        );
        let later = origin + SECOND * 3;
        assert_eq!(rate.refusing(later, Duration::ZERO, None), Some(SECOND * 2));
        assert_eq!(arriving(&rate, later), Err(SECOND * 2), "as arriving says");
        assert_eq!(
            rate.refusing(origin + SECOND * 5, Duration::ZERO, None),
            None
        );

        let waiting = Rate::new(1, WINDOW, Some(WINDOW));
        assert_eq!(arriving(&waiting, origin), Ok(()));
        assert_eq!(
            waiting.refusing(origin, Duration::ZERO, None),
            None,
            "the token 10 s away is waited for"
        );
        assert_eq!(
            waiting.refusing(origin, Duration::ZERO, Some(origin + WINDOW)),
            Some(WINDOW),
            "a token at the unit's deadline leaves its call no time"
        );
        assert_eq!(
            waiting.refusing(origin, Duration::ZERO, Some(origin + WINDOW + NANO)),
            None
        );
    }

    /// A read stands behind the attempts waiting, as an attempt arriving
    /// does: the token it tells is the one after theirs, though the one they
    /// wait for comes within the wait.
    #[tokio::test(start_paused = true)]
    async fn a_read_of_the_rate_stands_behind_the_attempts_waiting() {
        let rate = Rate::new(1, WINDOW, Some(SECOND * 15));
        let origin = Instant::now();
        assert_eq!(arriving(&rate, origin), Ok(()));
        let (waiting, due) = reserved(&rate, origin);
        assert_eq!(due, origin + WINDOW);
        let later = origin + SECOND;
        assert_eq!(
            rate.refusing(later, Duration::ZERO, None),
            Some(SECOND * 19),
            "the token at 20 s, behind the one waiting for the token 9 s away"
        );
        assert!(
            matches!(rate.arrive(later, None), Arrived::Refused(wait) if wait == SECOND * 19),
            "as arriving says"
        );
        drop(waiting);
        assert_eq!(rate.refusing(later, Duration::ZERO, None), None);
    }

    /// A call that arrives for its token later — once a slot of the bulkhead
    /// frees — is refused by the read only for a token no wait it may make
    /// reaches: one past its latest arrival and the rate's wait from there,
    /// to the nanosecond, or at its unit's deadline, however long it may
    /// wait. With no wait of the rate's, the token is free by its latest
    /// arrival or the call is refused.
    #[tokio::test(start_paused = true)]
    async fn a_read_for_a_call_arriving_later_refuses_only_a_token_no_wait_reaches() {
        let rate = Rate::new(1, WINDOW, Some(SECOND * 15));
        let origin = Instant::now();
        assert_eq!(arriving(&rate, origin), Ok(()));
        let (_waiting, _) = reserved(&rate, origin);
        let now = origin + SECOND;
        let token = origin + WINDOW * 2;
        let away = Some(SECOND * 19);
        assert_eq!(rate.refusing(now, Duration::ZERO, None), away);
        assert_eq!(rate.refusing(now, SECOND * 4 - NANO, None), away);
        assert_eq!(
            rate.refusing(now, SECOND * 4, None),
            None,
            "arriving 4 s on at the latest, 15 s from its token"
        );
        assert_eq!(rate.refusing(now, SECOND * 60, None), None);
        assert_eq!(
            rate.refusing(now, SECOND * 60, Some(token)),
            away,
            "a token at the unit's deadline leaves its call no time, whatever it may wait"
        );
        assert_eq!(rate.refusing(now, SECOND * 60, Some(token + NANO)), None);

        let unwaited = Rate::new(1, WINDOW, None);
        assert_eq!(arriving(&unwaited, origin), Ok(()));
        assert_eq!(
            unwaited.refusing(now, SECOND * 9 - NANO, None),
            Some(SECOND * 9)
        );
        assert_eq!(
            unwaited.refusing(now, SECOND * 9, None),
            None,
            "the token is free as it arrives at the latest"
        );
    }

    /// An attempt waits for a token that comes within the rate's wait and
    /// before its unit's deadline, and for no other: a token past either is
    /// refused, nothing reserved, so the next attempt is told the same one.
    #[tokio::test(start_paused = true)]
    async fn a_token_past_the_wait_or_at_the_units_deadline_is_refused_and_not_reserved() {
        let rate = Rate::new(1, WINDOW, Some(SECOND * 6));
        let origin = Instant::now();
        assert_eq!(arriving(&rate, origin), Ok(()));
        let refused = |at, deadline| match rate.arrive(at, deadline) {
            Arrived::Refused(wait) => Some(wait),
            Arrived::Taken | Arrived::Reserved(..) => None,
        };
        assert_eq!(refused(origin + SECOND * 3, None), Some(SECOND * 7));
        assert_eq!(
            refused(origin + SECOND * 5, Some(origin + WINDOW)),
            Some(SECOND * 5),
            "within the wait, at the deadline"
        );
        let (first, due) = reserved(&rate, origin + SECOND * 4);
        assert_eq!(due, origin + WINDOW, "6 s away, the whole wait");
        assert_eq!(
            refused(origin + SECOND * 9, None),
            Some(SECOND * 11),
            "the token after the one reserved, 11 s away"
        );
        drop(first);
        assert_eq!(
            refused(origin + SECOND * 3, None),
            Some(SECOND * 7),
            "given back, the token at 10 s is the next attempt's again"
        );
    }

    /// Attempts finding no token free are served in the order they arrived,
    /// each due the token after the last one's, and an attempt arriving stands
    /// behind them all: it takes no token an attempt in line is due, though
    /// that one has yet to take it.
    #[tokio::test(start_paused = true)]
    async fn attempts_waiting_are_served_in_arrival_order_each_at_its_own_token() {
        let rate = Rate::new(2, WINDOW, Some(WINDOW * 3));
        let origin = Instant::now();
        let at = |secs| origin + SECOND * secs;
        assert_eq!(arriving(&rate, origin), Ok(()));
        assert_eq!(arriving(&rate, origin), Ok(()));
        let (first, first_due) = reserved(&rate, at(1));
        let (second, second_due) = reserved(&rate, at(1));
        let (third, third_due) = reserved(&rate, at(2));
        assert_eq!([first_due, second_due, third_due], [at(5), at(10), at(15)]);
        assert_eq!(rate.available(at(12)), 0, "both tokens back are the line's");

        let Err((second, told)) = second.take(at(5)) else {
            panic!("the second in line took the first's token");
        };
        assert_eq!(told, at(10));
        assert!(first.take(at(5)).is_ok());
        let late = at(12);
        assert!(
            matches!(rate.arrive(late, None), Arrived::Reserved(_, due) if due == at(20)),
            "at 12 s the second has yet to take its token, due at 10 s: an attempt arriving \
             is behind the third's"
        );
        assert!(second.take(late).is_ok());
        assert!(
            matches!(third.take(late), Err((_, due)) if due == at(15)),
            "the second, late, took nothing of the third's"
        );
    }

    /// A reservation dropped gives its token to the attempt behind it, and
    /// every attempt behind moves up one token, woken to arm its timer anew;
    /// the token the last one leaves goes to the next attempt to arrive.
    #[tokio::test(start_paused = true)]
    async fn a_reservation_dropped_gives_its_token_to_the_next_in_line_and_wakes_it() {
        let rate = Rate::new(1, WINDOW, Some(WINDOW * 4));
        let origin = Instant::now();
        let at = |secs| origin + SECOND * secs;
        assert_eq!(arriving(&rate, origin), Ok(()));
        let (first, _) = reserved(&rate, origin);
        let (second, _) = reserved(&rate, origin);
        let (third, _) = reserved(&rate, origin);
        let (fourth, fourth_due) = reserved(&rate, origin);
        assert_eq!(fourth_due, at(40));
        let woken: [Arc<Woken>; 4] = std::array::from_fn(|_| Arc::default());
        let wakers = woken.each_ref().map(|woken| Waker::from(Arc::clone(woken)));
        let dues =
            |line: [&Reservation<'_>; 3]| [0, 1, 2].map(|nth| line[nth].due(&wakers[nth]) - origin);
        assert_eq!(
            dues([&first, &third, &fourth]),
            [WINDOW, WINDOW * 3, WINDOW * 4]
        );
        assert_eq!(second.due(&wakers[3]), at(20));

        drop(second);
        assert_eq!(
            [0, 1, 2, 3].map(|nth| woken[nth].times()),
            [0, 1, 1, 0],
            "the two behind it, and neither the one before nor itself"
        );
        assert_eq!(
            dues([&first, &third, &fourth]),
            [WINDOW, WINDOW * 2, WINDOW * 3],
            "each behind is due the token of the one before it"
        );
        assert!(
            matches!(rate.arrive(origin, None), Arrived::Reserved(_, due) if due == at(40)),
            "the token the fourth left, at 40 s, is the next attempt's"
        );
        assert!(first.take(at(10)).is_ok());
        assert!(third.take(at(20)).is_ok(), "at the dropped one's instant");
        assert!(fourth.take(at(30)).is_ok());
        assert_eq!(
            [0, 1, 2].map(|nth| woken[nth].times()),
            [0, 1, 1],
            "a token taken moves no one behind it"
        );
    }

    /// The wait an attempt is told as it reserves its token is the wait it
    /// waits, however late the attempts before it take theirs and whatever
    /// arrives meanwhile: the token it reserved stays due at its instant.
    #[tokio::test(start_paused = true)]
    async fn the_wait_an_attempt_is_told_is_the_wait_it_waits() {
        let rate = Rate::new(1, WINDOW, Some(WINDOW * 2));
        let origin = Instant::now();
        assert_eq!(arriving(&rate, origin), Ok(()));
        let (first, first_due) = reserved(&rate, origin);
        let (second, second_due) = reserved(&rate, origin);
        assert_eq!(
            [first_due, second_due],
            [origin + WINDOW, origin + WINDOW * 2]
        );

        tokio::time::sleep_until(first_due).await;
        tokio::time::advance(Duration::from_millis(7)).await;
        assert!(first.take(Instant::now()).is_ok(), "7 ms late");
        tokio::time::sleep_until(second_due).await;
        assert_eq!(Instant::now(), second_due);
        assert_eq!(
            arriving(&rate, Instant::now()),
            Err(WINDOW),
            "the token due now is the second's"
        );
        assert!(
            second.take(Instant::now()).is_ok(),
            "at the instant it was told, not 7 ms past it"
        );
        assert_eq!(arriving(&rate, Instant::now()), Err(WINDOW));
    }

    /// An attempt so late that the bucket filled behind its token keeps that
    /// token, set aside from the bucket's: it and the bucket's limit go out
    /// together, and the next token comes an interval later.
    #[tokio::test(start_paused = true)]
    async fn a_token_reserved_is_set_aside_from_the_bucket_filling_behind_it() {
        let rate = Rate::new(2, WINDOW, Some(WINDOW));
        let origin = Instant::now();
        let interval = WINDOW / 2;
        assert_eq!(arriving(&rate, origin), Ok(()));
        assert_eq!(arriving(&rate, origin), Ok(()));
        let (late, due) = reserved(&rate, origin);
        assert_eq!(due, origin + interval);
        let woke = origin + WINDOW * 5;
        assert_eq!(rate.available(woke), 2, "the bucket's own, full");
        assert_eq!(arriving(&rate, woke), Ok(()));
        assert!(
            late.take(woke).is_ok(),
            "still due, an arrival taken before it"
        );
        assert_eq!(arriving(&rate, woke), Ok(()));
        assert_eq!(arriving(&rate, woke), Err(interval));
    }

    /// An attempt whose clock was read before another took the last token —
    /// one reading the clock later, on another worker, took it first — is
    /// told the time from its own read to the next token.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_read_before_the_last_token_was_taken_is_told_from_its_own_read() {
        let rate = Rate::new(1, WINDOW, None);
        let earlier = Instant::now();
        assert_eq!(arriving(&rate, earlier + SECOND), Ok(()));
        let to_the_token = WINDOW + SECOND;
        assert_eq!(arriving(&rate, earlier), Err(to_the_token));
        assert_eq!(
            rate.refusing(earlier, Duration::ZERO, None),
            Some(to_the_token)
        );
    }

    /// A window of no time, which the config refuses, refills the bucket at
    /// every instant, so no limit refuses an attempt, as the window's floor
    /// says.
    #[tokio::test(start_paused = true)]
    async fn a_window_of_no_time_refuses_none() {
        let rate = Rate::new(1, Duration::ZERO, None);
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(arriving(&rate, now), Ok(()));
        }
        assert_eq!(rate.refusing(now, Duration::ZERO, None), None);
        assert_eq!(rate.available(now), 1);
        assert_eq!(rate.most_sent_within(SECOND), usize::MAX);
    }

    /// At the config's ceiling, a billion a second, a token comes every
    /// nanosecond, and at the smallest window code takes, a nanosecond, a
    /// billion come in one: an interval under a nanosecond is kept whole, and
    /// a wait for a part of one is told as a nanosecond, never as none.
    #[tokio::test(start_paused = true)]
    async fn an_interval_at_or_under_a_nanosecond_is_kept_whole() {
        const BILLION: usize = 1_000_000_000;
        let origin = Instant::now();

        let per_second = Rate::new(BILLION, SECOND, None);
        for _ in 0..1_000 {
            assert_eq!(arriving(&per_second, origin), Ok(()));
        }
        for (after, free) in [
            (Duration::ZERO, BILLION - 1_000),
            (NANO, BILLION - 999),
            (NANO * 999, BILLION - 1),
            (NANO * 1_000, BILLION),
            (SECOND, BILLION),
        ] {
            assert_eq!(per_second.available(origin + after), free, "{after:?} on");
        }
        assert_eq!(per_second.most_sent_within(NANO), BILLION + 1);
        assert_eq!(per_second.most_sent_within(DAY), BILLION + 86_400 * BILLION);

        let per_nano = Rate::new(BILLION, NANO, None);
        for _ in 0..1_000 {
            assert_eq!(arriving(&per_nano, origin), Ok(()));
        }
        assert_eq!(per_nano.available(origin), BILLION - 1_000);
        assert_eq!(
            per_nano.available(origin + NANO),
            BILLION,
            "a billion in one"
        );
        assert_eq!(per_nano.most_sent_within(NANO), 2 * BILLION);

        let thirds = Rate::new(3, NANO, None);
        for _ in 0..3 {
            assert_eq!(arriving(&thirds, origin), Ok(()));
        }
        assert_eq!(
            arriving(&thirds, origin),
            Err(NANO),
            "a third of a nanosecond away, told as one"
        );
        assert_eq!(thirds.available(origin + NANO), 3);
    }

    /// One token a day comes a day after the last, to the nanosecond, and
    /// seven a day, an interval no whole number of nanoseconds, come at each
    /// seventh of the day rounded up, day after day: the seventh of each day
    /// on the day itself, nothing drifting.
    #[tokio::test(start_paused = true)]
    async fn a_token_a_day_comes_to_the_nanosecond_and_nothing_drifts_over_days() {
        let origin = Instant::now();
        let daily = Rate::new(1, DAY, None);
        assert_eq!(arriving(&daily, origin), Ok(()));
        assert_eq!(arriving(&daily, origin), Err(DAY));
        assert_eq!(arriving(&daily, origin + DAY - NANO), Err(NANO));
        assert_eq!(arriving(&daily, origin + DAY), Ok(()));
        assert_eq!(arriving(&daily, origin + DAY), Err(DAY));

        let rate = Rate::new(7, DAY, None);
        for _ in 0..7 {
            assert_eq!(arriving(&rate, origin), Ok(()));
        }
        for nth in 1..=21_u128 {
            let exact = DAY.as_nanos() * nth;
            let due = origin + Duration::from_nanos_u128(exact.div_ceil(7));
            if !exact.is_multiple_of(7) {
                assert_eq!(arriving(&rate, due - NANO), Err(NANO), "token {nth}");
            }
            assert_eq!(arriving(&rate, due), Ok(()), "token {nth}");
        }
        assert_eq!(
            arriving(&rate, origin + DAY * 3),
            Err(Duration::from_nanos_u128(DAY.as_nanos().div_ceil(7))),
            "the twenty-first token was due at three days to the nanosecond"
        );
    }

    /// The whole tokens free are what as many attempts arriving together
    /// take, and none while an attempt waits for one, whose token the next
    /// one back is.
    #[tokio::test(start_paused = true)]
    async fn the_tokens_read_free_are_the_ones_attempts_arriving_would_take() {
        let rate = Rate::new(3, WINDOW, Some(WINDOW));
        let origin = Instant::now();
        for secs in [0, 2, 4, 7, 11, 30] {
            let now = origin + SECOND * secs;
            let free = rate.available(now);
            for _ in 0..free {
                assert_eq!(arriving(&rate, now), Ok(()), "at {secs} s, of {free} free");
            }
            assert!(arriving(&rate, now).is_err(), "at {secs} s, past {free}");
            assert_eq!(rate.available(now), 0);
        }
        let now = origin + SECOND * 30;
        let (waiting, due) = reserved(&rate, now);
        assert_eq!(
            rate.available(due),
            0,
            "the token back is the one waited for"
        );
        assert_eq!(rate.available(due + WINDOW / 3), 1);
        drop(waiting);
        assert_eq!(rate.available(due), 1);
    }

    /// How many attempts `rate` sends within `span`, both ends included, each
    /// sent as soon as the bucket lets it through, from a start the bucket is
    /// full at, or with its `taken` tokens taken `before` it.
    fn sent_within(rate: &Rate, span: Duration, drained: Option<(usize, Duration)>) -> usize {
        let starts = Instant::now() + DAY;
        if let Some((taken, before)) = drained {
            for _ in 0..taken {
                assert_eq!(arriving(rate, starts - before), Ok(()), "drains a token");
            }
        }
        let ends = starts + span;
        let (mut now, mut sent) = (starts, 0);
        loop {
            match arriving(rate, now) {
                Ok(()) => sent += 1,
                Err(wait) if now + wait <= ends => now += wait,
                Err(_) => return sent,
            }
        }
    }

    /// The most attempts a span holds are the most the bucket sends in it from
    /// any start: its limit from a bucket full as the span starts, then a
    /// token each time one comes before it ends, the last at its end. A bucket
    /// part drained as the span starts sends fewer. Two per 10 s send three
    /// within 5 s, six within 20 s and seven within 25 s; three per 10 s,
    /// whose interval is no whole number of nanoseconds, send six within 10 s
    /// and five within a nanosecond less.
    #[tokio::test(start_paused = true)]
    async fn the_most_attempts_a_span_holds_are_the_most_the_bucket_sends_in_it_from_any_start() {
        let rate = |limit| Rate::new(limit, WINDOW, None);
        let most_sent = |limit: usize, span| {
            let mut starts = vec![None];
            for taken in 1..=limit {
                for before in [Duration::ZERO, NANO, WINDOW / 7, WINDOW / 2, WINDOW - NANO] {
                    starts.push(Some((taken, before)));
                }
            }
            let sent: Vec<usize> = starts
                .into_iter()
                .map(|drained| sent_within(&rate(limit), span, drained))
                .collect();
            assert_eq!(
                sent.first(),
                sent.iter().max(),
                "a full bucket sends the most: {sent:?}"
            );
            sent.into_iter().max()
        };
        assert_eq!(most_sent(2, SECOND * 5), Some(3));
        assert_eq!(most_sent(2, SECOND * 20), Some(6));
        assert_eq!(most_sent(2, SECOND * 25), Some(7));
        assert_eq!(most_sent(3, WINDOW), Some(6));
        assert_eq!(most_sent(3, WINDOW - NANO), Some(5));
        for limit in 1..=4 {
            let interval = WINDOW / u32::try_from(limit).expect("a small limit");
            for whole in 0..=3 {
                for span in [
                    WINDOW * whole,
                    WINDOW * whole + NANO,
                    WINDOW * whole + interval - NANO,
                    WINDOW * whole + interval,
                    WINDOW * whole + interval + NANO,
                    WINDOW * whole + WINDOW / 2,
                    WINDOW * (whole + 1) - NANO,
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
}
