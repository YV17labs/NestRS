//! When a decorated method drops what it holds, each form beside the
//! undecorated method written with the same tokens: its arguments, in the
//! order an `async fn` drops its parameters however its run ends, and what
//! its body's tail expression leaves, where its crate's edition says.

use super::*;

/// What dropped, in the order it did.
#[derive(Clone, Default)]
struct Drops(Arc<Mutex<Vec<&'static str>>>);

impl Drops {
    /// A value writing `name` here when it drops.
    fn noted(&self, name: &'static str) -> Noted {
        Noted(name, self.clone())
    }

    /// The arguments dropped since the last read, a body's local left out.
    fn arguments(&self) -> Vec<&'static str> {
        let mut dropped = std::mem::take(&mut *self.0.lock().expect("unpoisoned"));
        dropped.retain(|name| *name != "local");
        dropped
    }
}

struct Noted(&'static str, Drops);

impl Noted {
    /// A local of the body reading it, dropped ahead of every argument.
    fn beside(&self) -> Self {
        self.1.noted("local")
    }

    fn name(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Noted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.0)
    }
}

impl Drop for Noted {
    fn drop(&mut self) {
        self.1.0.lock().expect("unpoisoned").push(self.0);
    }
}

/// How a body ends.
#[derive(Clone, Copy, Debug)]
enum Ends {
    Answered,
    Failed,
    Panicked,
    Waiting,
}

impl Ends {
    async fn answer(self) -> Result<(), CarrierDown> {
        match self {
            Self::Answered => Ok(()),
            Self::Failed => Err(CarrierDown),
            Self::Panicked => panic!("the body panicked"),
            Self::Waiting => std::future::pending().await,
        }
    }
}

/// Whether the lock it reads was held when it dropped.
struct Publish<'a>(&'a Mutex<u32>, &'a AtomicBool);

impl Drop for Publish<'_> {
    fn drop(&mut self) {
        self.1.store(self.0.try_lock().is_err(), Ordering::Relaxed);
    }
}

fn seated(seats: u32) -> Result<u32, CarrierDown> {
    Ok(seats)
}

struct Ledger {
    carrier: Arc<ResiliencePolicy>,
    /// The lock a tail expression takes, which a local reads in its drop.
    seats: Mutex<u32>,
}

/// One method written twice, `$plain` undecorated and `$decorated` under
/// its policy: the tokens reach both as written.
macro_rules! twins {
    (
        #[resilient($($policy:tt)*)]
        async fn $plain:ident, $decorated:ident ($($arguments:tt)*) -> $answered:ty $body:block
    ) => {
        async fn $plain($($arguments)*) -> Result<$answered, CarrierDown> $body

        #[resilient($($policy)*)]
        async fn $decorated($($arguments)*) -> Answer<$answered> $body
    };
}

impl Ledger {
    twins! {
        #[resilient(carrier)]
        async fn plain, once(&self, held: Noted, out: Noted, ends: Ends) -> () {
            let _local = held.beside();
            out.name();
            ends.answer().await
        }
    }

    twins! {
        #[resilient(carrier)]
        async fn plain_out_first, once_out_first(
            &self,
            held: Noted,
            out: Noted,
            ends: Ends,
        ) -> () {
            out.name();
            let _local = held.beside();
            ends.answer().await
        }
    }

    #[resilient(carrier, cancel = token)]
    async fn once_reading_its_token(&self, held: Noted, out: Noted, ends: Ends) -> Answer<()> {
        drop(token);
        let _local = held.beside();
        out.name();
        ends.answer().await
    }

    // Each argument named by a format string alone, one in its position
    // and one in another's width.
    twins! {
        #[resilient(carrier)]
        async fn plain_formatted, once_formatted(
            &self,
            held: Noted,
            out: usize,
            ends: Ends,
            last: Noted,
        ) -> () {
            assert_eq!(format!("{held}{:out$}", 1), "held   1");
            assert_eq!(format!("{{last}}{last:>5}"), "{last} last");
            ends.answer().await
        }
    }

    twins! {
        #[resilient(carrier, idempotent)]
        async fn plain_again, again(&self, held: Noted, out: Noted, ends: Ends) -> () {
            let _local = held.beside();
            out.name();
            ends.answer().await
        }
    }

    twins! {
        #[resilient(carrier, record_if = |_error| !held.name().is_empty())]
        async fn plain_judged, judged(&self, held: Noted, out: Noted, ends: Ends) -> () {
            let _local = held.beside();
            out.name();
            ends.answer().await
        }
    }

    // Both arguments consumed, past where the body may end.
    twins! {
        #[resilient(carrier, record_if = |_error| true)]
        async fn plain_handing_on, judged_handing_on(
            &self,
            held: Noted,
            out: Noted,
            ends: Ends,
        ) -> () {
            ends.answer().await?;
            Self::handed(held, out)
        }
    }

    // The first argument consumed on one path, the others only read.
    twins! {
        #[resilient(carrier, record_if = |_error| true)]
        async fn plain_handing_one_on, judged_handing_one_on(
            &self,
            held: Noted,
            lent: Noted,
            out: Noted,
            ends: Ends,
        ) -> () {
            if matches!(ends, Ends::Answered) {
                drop(held);
            }
            lent.name();
            out.name();
            ends.answer().await
        }
    }

    // Both consumed, named last declared first in a statement of the body's own.
    twins! {
        #[resilient(carrier, record_if = |_error| true)]
        async fn plain_naming_first, judged_naming_first(
            &self,
            held: Noted,
            out: Noted,
            ends: Ends,
        ) -> () {
            let _ = (&out, &held);
            ends.answer().await?;
            Self::handed(held, out)
        }
    }

    fn handed(held: Noted, out: Noted) -> Result<(), CarrierDown> {
        held.name();
        out.name();
        Ok(())
    }

    twins! {
        #[resilient(carrier, fallback = recovered)]
        async fn plain_recovering, recovering(
            &self,
            held: Noted,
            out: Noted,
            ends: Ends,
        ) -> () {
            let _local = held.beside();
            out.name();
            ends.answer().await
        }
    }

    async fn recovered(
        &self,
        held: Noted,
        out: Noted,
        _ends: Ends,
        _error: ResilienceError<CarrierDown>,
    ) -> Answer<()> {
        held.name();
        out.name();
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn traced_plain(&self, held: Noted, out: Noted, ends: Ends) -> Result<(), CarrierDown> {
        let _local = held.beside();
        out.name();
        ends.answer().await
    }

    #[tracing::instrument(skip_all)]
    #[resilient(carrier)]
    async fn traced_above(&self, held: Noted, out: Noted, ends: Ends) -> Answer<()> {
        let _local = held.beside();
        out.name();
        ends.answer().await
    }

    #[resilient(carrier)]
    #[tracing::instrument(skip_all)]
    async fn traced_below(&self, held: Noted, out: Noted, ends: Ends) -> Answer<()> {
        let _local = held.beside();
        out.name();
        ends.answer().await
    }

    // The middle argument consumed.
    twins! {
        #[resilient(carrier)]
        async fn plain_consuming, once_consuming(
            &self,
            held: Noted,
            sent: Noted,
            out: Noted,
        ) -> () {
            held.name();
            drop(sent);
            out.name();
            Ok(())
        }
    }

    // A pattern's unnamed half and an argument the body names nowhere.
    twins! {
        #[resilient(carrier)]
        async fn plain_in_part, once_in_part(
            &self,
            (first, _): (Noted, Noted),
            _unread: Noted,
            last: Noted,
        ) -> () {
            first.name();
            last.name();
            Ok(())
        }
    }

    // An argument named with a leading `_`, which the body names all the same.
    twins! {
        #[resilient(carrier)]
        async fn plain_underscored, once_underscored(
            &self,
            held: Noted,
            _kept: Noted,
            last: Noted,
        ) -> () {
            _kept.name();
            held.name();
            last.name();
            Ok(())
        }
    }

    twins! {
        #[resilient(carrier)]
        async fn plain_seated, seated_once(&self, under_lock: &AtomicBool) -> u32 {
            let _publish = Publish(&self.seats, under_lock);
            seated(*self.seats.lock().expect("unpoisoned"))
        }
    }

    #[resilient(carrier, idempotent, fallback = unseated, cancel = token)]
    async fn seated_again(&self, under_lock: &AtomicBool) -> Answer<u32> {
        let _publish = Publish(&self.seats, under_lock);
        drop(token);
        seated(*self.seats.lock().expect("unpoisoned"))
    }

    async fn unseated(
        &self,
        _under_lock: &AtomicBool,
        _error: ResilienceError<CarrierDown>,
    ) -> Answer<u32> {
        Ok(0)
    }

    /// A local its tail expression borrows: in this crate's edition what
    /// the tail leaves drops first, and the method compiles as its
    /// undecorated twin does.
    #[resilient(carrier)]
    async fn borrowed_in_the_tail(&self) -> Answer<u32> {
        let seats = std::cell::RefCell::new(4);
        Ok(*seats.borrow())
    }
}

async fn ledger(config: ResilienceConfig) -> (HeadlessApp, Arc<Ledger>) {
    let (app, carrier) = crate::policy(config).await;
    let ledger = Ledger {
        carrier,
        seats: Mutex::new(4),
    };
    (app, Arc::new(ledger))
}

/// The arguments `run` dropped, in order, once it answered, panicked or
/// was dropped a second in, on the paused clock.
async fn dropped_by<Fut>(drops: &Drops, run: Fut) -> Vec<&'static str>
where
    Fut: Future + Send + 'static,
    Fut::Output: Send,
{
    let run = tokio::spawn(tokio::time::timeout(Duration::from_secs(1), run));
    let _answered_panicked_or_cut = run.await;
    drops.arguments()
}

const ENDS: [Ends; 4] = [Ends::Answered, Ends::Failed, Ends::Panicked, Ends::Waiting];

/// `(held, out)` as an `async fn` drops them: last declared first.
const LAST_DECLARED_FIRST: [&str; 2] = ["out", "held"];

/// What `$method` of `$ledger` drops of `(held, out)`, ending as `$ends`.
macro_rules! dropped_by_method {
    ($ledger:ident . $method:ident, $ends:expr) => {{
        let drops = Drops::default();
        let (ledger, held, out) = (
            Arc::clone(&$ledger),
            drops.noted("held"),
            drops.noted("out"),
        );
        dropped_by(
            &drops,
            async move { ledger.$method(held, out, $ends).await },
        )
        .await
    }};
}

macro_rules! each_twin_drops_alike {
    ($ledger:ident, $ends:ident: $($plain:ident, $decorated:ident;)*) => {$(
        let plain = dropped_by_method!($ledger.$plain, $ends);
        assert_eq!(plain, LAST_DECLARED_FIRST, "{} ending {:?}", stringify!($plain), $ends);
        let decorated = dropped_by_method!($ledger.$decorated, $ends);
        assert_eq!(decorated, plain, "{} ending {:?}", stringify!($decorated), $ends);
    )*};
}

/// A lock argument is released after the writer declared after it flushed,
/// under a policy as with none: run once, the attempt owning the
/// arguments; lending them to the next attempt or to a predicate, its body
/// reading both, or consuming both it first names in that order; and
/// handing them to a fallback, called or not.
#[tokio::test(start_paused = true)]
async fn a_method_drops_its_arguments_last_declared_first_however_its_run_ends() {
    let (_app, ledger) = ledger(quiet()).await;
    for ends in ENDS {
        each_twin_drops_alike! {
            ledger, ends:
            plain, once;
            plain_out_first, once_out_first;
            plain, once_reading_its_token;
            plain_again, again;
            plain_judged, judged;
            plain_naming_first, judged_naming_first;
            plain_recovering, recovering;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_call_refused_unsent_drops_its_arguments_last_declared_first() {
    let (_app, ledger) = ledger(ResilienceConfig {
        retries: None,
        ..ResilienceConfig::default()
    })
    .await;
    ledger
        .carrier
        .open_breaker(Duration::from_secs(30))
        .expect("an open time the breaker takes");
    for dropped in [
        dropped_by_method!(ledger.once, Ends::Answered),
        dropped_by_method!(ledger.once_out_first, Ends::Answered),
        dropped_by_method!(ledger.once_reading_its_token, Ends::Answered),
        dropped_by_method!(ledger.again, Ends::Answered),
        dropped_by_method!(ledger.judged, Ends::Answered),
        dropped_by_method!(ledger.judged_naming_first, Ends::Answered),
        dropped_by_method!(ledger.recovering, Ends::Answered),
    ] {
        assert_eq!(dropped, LAST_DECLARED_FIRST);
    }
}

/// An instrument's own future takes the arguments it is written over.
/// Above a method run once it takes them from the attempt, which then
/// fixes no order: they drop as the instrumented method's do with no
/// policy, sent or not. Below, it takes them in the order the expansion
/// names them, the attempt's.
#[tokio::test(start_paused = true)]
async fn beside_an_instrument_the_arguments_drop_in_the_order_its_future_takes_them() {
    let (_app, ledger) = ledger(ResilienceConfig {
        retries: None,
        ..ResilienceConfig::default()
    })
    .await;
    for ends in ENDS {
        let plain = dropped_by_method!(ledger.traced_plain, ends);
        assert_eq!(
            dropped_by_method!(ledger.traced_above, ends),
            plain,
            "{ends:?}"
        );
        assert_eq!(
            dropped_by_method!(ledger.traced_below, ends),
            LAST_DECLARED_FIRST,
            "{ends:?}"
        );
    }
    ledger
        .carrier
        .open_breaker(Duration::from_secs(30))
        .expect("an open time the breaker takes");
    assert_eq!(
        dropped_by_method!(ledger.traced_above, Ends::Answered),
        dropped_by_method!(ledger.traced_plain, Ends::Answered),
    );
    assert_eq!(
        dropped_by_method!(ledger.traced_below, Ends::Answered),
        LAST_DECLARED_FIRST
    );
}

#[tokio::test(start_paused = true)]
async fn an_argument_a_format_string_alone_names_drops_in_its_turn() {
    let (_app, ledger) = ledger(quiet()).await;
    for ends in ENDS {
        let mut twins = Vec::new();
        for decorated in [false, true] {
            let drops = Drops::default();
            let (ledger, held, last) = (
                Arc::clone(&ledger),
                drops.noted("held"),
                drops.noted("last"),
            );
            twins.push(
                dropped_by(&drops, async move {
                    match decorated {
                        false => ledger.plain_formatted(held, 4, ends, last).await.is_ok(),
                        true => ledger.once_formatted(held, 4, ends, last).await.is_ok(),
                    }
                })
                .await,
            );
        }
        assert_eq!(twins[0], ["last", "held"], "{ends:?}");
        assert_eq!(twins[1], twins[0], "{ends:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn an_argument_the_body_consumes_leaves_the_others_their_order() {
    let (_app, ledger) = ledger(quiet()).await;
    let mut twins = Vec::new();
    for decorated in [false, true] {
        let drops = Drops::default();
        let (ledger, held, sent, out) = (
            Arc::clone(&ledger),
            drops.noted("held"),
            drops.noted("sent"),
            drops.noted("out"),
        );
        twins.push(
            dropped_by(&drops, async move {
                match decorated {
                    false => ledger.plain_consuming(held, sent, out).await.is_ok(),
                    true => ledger.once_consuming(held, sent, out).await.is_ok(),
                }
            })
            .await,
        );
    }
    assert_eq!(twins[0], ["sent", "out", "held"]);
    assert_eq!(twins[1], twins[0]);
}

/// What `$method` of `$ledger` drops of `(held, lent, out)`, ending as `$ends`.
macro_rules! dropped_of_three_by_method {
    ($ledger:ident . $method:ident, $ends:expr) => {{
        let drops = Drops::default();
        let (ledger, held, lent, out) = (
            Arc::clone(&$ledger),
            drops.noted("held"),
            drops.noted("lent"),
            drops.noted("out"),
        );
        dropped_by(&drops, async move {
            ledger.$method(held, lent, out, $ends).await
        })
        .await
    }};
}

/// The one difference `record_if` keeps: an argument the body consumes is
/// the attempt's own. One the body has not consumed when the attempt ends
/// drops with it — in the order the body first names them, and ahead of
/// those it only reads, which stay in the method's frame.
#[tokio::test(start_paused = true)]
async fn under_record_if_an_argument_the_body_would_consume_drops_with_the_attempt() {
    let (_app, ledger) = ledger(ResilienceConfig {
        retries: None,
        ..ResilienceConfig::default()
    })
    .await;
    for ends in ENDS {
        let consumed = matches!(ends, Ends::Answered);
        assert_eq!(
            dropped_by_method!(ledger.plain_handing_on, ends),
            LAST_DECLARED_FIRST,
            "{ends:?}"
        );
        assert_eq!(
            dropped_by_method!(ledger.judged_handing_on, ends),
            if consumed {
                LAST_DECLARED_FIRST
            } else {
                ["held", "out"]
            },
            "{ends:?}"
        );
        assert_eq!(
            dropped_of_three_by_method!(ledger.plain_handing_one_on, ends),
            if consumed {
                ["held", "out", "lent"]
            } else {
                ["out", "lent", "held"]
            },
            "{ends:?}"
        );
        assert_eq!(
            dropped_of_three_by_method!(ledger.judged_handing_one_on, ends),
            ["held", "out", "lent"],
            "{ends:?}"
        );
    }
    ledger
        .carrier
        .open_breaker(Duration::from_secs(30))
        .expect("an open time the breaker takes");
    assert_eq!(
        dropped_by_method!(ledger.judged_handing_on, Ends::Answered),
        ["held", "out"]
    );
    assert_eq!(
        dropped_of_three_by_method!(ledger.judged_handing_one_on, Ends::Answered),
        ["held", "out", "lent"]
    );
}

/// The one difference a method run once keeps: what its body does not
/// name stays in the method's own frame, and drops once the attempt has.
#[tokio::test(start_paused = true)]
async fn what_a_body_run_once_does_not_name_drops_after_its_attempt() {
    let (_app, ledger) = ledger(quiet()).await;
    let mut twins = Vec::new();
    for decorated in [false, true] {
        let drops = Drops::default();
        let ledger = Arc::clone(&ledger);
        let (pair, unread, last) = (
            (drops.noted("first"), drops.noted("rest")),
            drops.noted("unread"),
            drops.noted("last"),
        );
        twins.push(
            dropped_by(&drops, async move {
                match decorated {
                    false => ledger.plain_in_part(pair, unread, last).await.is_ok(),
                    true => ledger.once_in_part(pair, unread, last).await.is_ok(),
                }
            })
            .await,
        );
    }
    assert_eq!(twins[0], ["last", "unread", "first", "rest"]);
    assert_eq!(twins[1], ["last", "first", "unread", "rest"]);
}

/// The attempt reads no argument named with a leading `_`, a read clippy
/// reports: one the body names is the attempt's all the same, captured
/// where the body first names it, after the others.
#[tokio::test(start_paused = true)]
async fn an_argument_named_with_an_underscore_the_body_names_drops_after_the_others() {
    let (_app, ledger) = ledger(quiet()).await;
    let mut twins = Vec::new();
    for decorated in [false, true] {
        let drops = Drops::default();
        let (ledger, held, kept, last) = (
            Arc::clone(&ledger),
            drops.noted("held"),
            drops.noted("kept"),
            drops.noted("last"),
        );
        twins.push(
            dropped_by(&drops, async move {
                match decorated {
                    false => ledger.plain_underscored(held, kept, last).await.is_ok(),
                    true => ledger.once_underscored(held, kept, last).await.is_ok(),
                }
            })
            .await,
        );
    }
    assert_eq!(twins[0], ["last", "kept", "held"]);
    assert_eq!(twins[1], ["last", "held", "kept"]);
}

/// This crate is of edition 2024, where what a block's tail expression
/// leaves drops ahead of the block's locals: the lock is released by the
/// time the local reads it, decorated or not.
#[tokio::test(start_paused = true)]
async fn what_a_bodys_tail_leaves_drops_where_its_undecorated_twin_drops_it() {
    let (_app, ledger) = ledger(quiet()).await;
    let under_lock = AtomicBool::new(true);
    assert!(matches!(ledger.plain_seated(&under_lock).await, Ok(4)));
    assert!(!under_lock.swap(true, Ordering::Relaxed));
    assert!(matches!(ledger.seated_once(&under_lock).await, Ok(4)));
    assert!(!under_lock.swap(true, Ordering::Relaxed));
    assert!(matches!(ledger.seated_again(&under_lock).await, Ok(4)));
    assert!(!under_lock.swap(true, Ordering::Relaxed));
    assert!(matches!(ledger.borrowed_in_the_tail().await, Ok(4)));
}
