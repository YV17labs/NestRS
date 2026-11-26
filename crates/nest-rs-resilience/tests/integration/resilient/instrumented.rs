//! `#[tracing::instrument]` beside `#[resilient]`, in either order: written
//! above, tracing expands first and its span covers one attempt; written
//! below, the span covers the whole run. Each flavour of the instrument that
//! changes what it expands to is run in both orders, held to the attempts it
//! makes, the spans it opens and what each records.

use super::*;

#[injectable]
struct TracedService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    attempts: AtomicU32,
    hub: Hub,
}

/// One method written twice, `$above` under an instrument written above
/// its policy and `$below` under the same one written below it. The
/// method's tokens reach both as written, so each is the method a
/// developer types.
macro_rules! in_both_orders {
    (
        #[tracing::instrument($($traced:tt)*)]
        #[resilient($($policy:tt)*)]
        async fn $above:ident, $below:ident $($method:tt)*
    ) => {
        #[tracing::instrument($($traced)*)]
        #[resilient($($policy)*)]
        async fn $above $($method)*

        #[resilient($($policy)*)]
        #[tracing::instrument($($traced)*)]
        async fn $below $($method)*
    };
}

impl TracedService {
    fn fails_first(&self) -> bool {
        self.attempts.fetch_add(1, Ordering::Relaxed) == 0
    }

    fn attempted(&self) -> u32 {
        self.attempts.swap(0, Ordering::Relaxed)
    }

    #[tracing::instrument(name = "above", skip_all)]
    #[resilient(courier, idempotent)]
    async fn above(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(name = "below", skip_all)]
    async fn below(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    /// Run once, consuming its argument, its error recorded by the span.
    #[tracing::instrument(name = "above_once", skip_all, err)]
    #[resilient(courier)]
    async fn above_once(&self, parcel: String) -> Answer<String> {
        depot(&parcel)?;
        Ok(parcel)
    }

    /// Two instruments above the policy, one inside the other: each
    /// attempt opens both.
    #[tracing::instrument(name = "outer", skip_all)]
    #[tracing::instrument(name = "inner", skip_all, err)]
    #[resilient(courier, idempotent)]
    async fn twice_above(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    in_both_orders! {
        #[tracing::instrument(skip_all, err)]
        #[resilient(courier, idempotent)]
        async fn err_above, err_below(&self, parcel: String) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            depot(&parcel)?;
            Ok(parcel.clone())
        }
    }

    in_both_orders! {
        #[tracing::instrument(skip_all, ret)]
        #[resilient(courier, idempotent)]
        async fn ret_above, ret_below(&self, parcel: String) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(parcel.clone())
        }
    }

    in_both_orders! {
        #[tracing::instrument(skip_all, err, ret)]
        #[resilient(courier, idempotent)]
        async fn both_above, both_below(&self, parcel: String) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(parcel.clone())
        }
    }

    // An owned argument the span records and each attempt reads again,
    // beside a field computed from one.
    in_both_orders! {
        #[tracing::instrument(skip(self, zone), fields(weight = parcel.len()))]
        #[resilient(courier, idempotent)]
        async fn fields_above, fields_below(&self, parcel: String, zone: &str) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(format!("{parcel} to {zone}"))
        }
    }

    // Run once: the one attempt consumes the argument it was moved.
    in_both_orders! {
        #[tracing::instrument(skip_all, err, ret)]
        #[resilient(courier)]
        async fn once_above, once_below(&self, parcel: String) -> Answer<String> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            depot(&parcel)?;
            Ok(parcel)
        }
    }

    in_both_orders! {
        #[tracing::instrument(skip_all, ret)]
        #[resilient(courier, idempotent)]
        async fn generic_above, generic_below<T>(&self, parcel: T) -> Answer<T>
        where
            T: std::fmt::Debug + Clone + Sync,
        {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(parcel.clone())
        }
    }

    // The answer borrows its host or its argument through a lifetime the
    // method names.
    in_both_orders! {
        #[tracing::instrument(skip_all, err)]
        #[resilient(courier, idempotent)]
        async fn borrowed_above, borrowed_below<'a>(&'a self, zone: &'a str) -> Answer<&'a str> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(if zone.is_empty() { &self.hub.0 } else { zone })
        }
    }

    /// The parcel held at the hub, in place of the error its run ended on.
    async fn held(&self, parcel: String, _error: ResilienceError<CarrierDown>) -> Answer<String> {
        Ok(format!("{parcel} held"))
    }

    // Every other key: each predicate, a fallback handed the argument
    // each attempt read, and the attempt's token read in the body.
    in_both_orders! {
        #[tracing::instrument(skip_all, err)]
        #[resilient(
            courier,
            idempotent,
            retry_if = |_| true,
            retry_ok_if = String::is_empty,
            record_if = |_| true,
            fallback = held,
            fallback_if = |_| true,
            cancel = token
        )]
        async fn keyed_above, keyed_below(&self, parcel: String) -> Answer<String> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if token.is_cancelled() {
                return Ok(String::new());
            }
            depot(&parcel)?;
            Ok(parcel.clone())
        }
    }

    // Sent once, its one attempt lending the argument its fallback takes.
    in_both_orders! {
        #[tracing::instrument(skip_all, ret)]
        #[resilient(courier, fallback = held, cancel = token)]
        async fn lent_above, lent_below(&self, parcel: String) -> Answer<String> {
            let _attempt = token;
            depot(&parcel)?;
            Ok(parcel.clone())
        }
    }

    // Every predicate reading the argument the method owns, which its
    // fallback is then handed by value.
    in_both_orders! {
        #[tracing::instrument(skip_all)]
        #[resilient(
            courier,
            idempotent,
            retry_if = |_| parcel != "P0",
            retry_ok_if = |held: &String| held.len() > parcel.len(),
            record_if = |_| !parcel.is_empty(),
            fallback = held,
            fallback_if = |_| parcel != "P0",
            cancel = token
        )]
        async fn read_above, read_below(&self, parcel: String) -> Answer<String> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            drop(token);
            depot(&parcel)?;
            Ok(parcel.clone())
        }
    }

    // Sent once under a predicate reading the argument its one attempt
    // lends it.
    in_both_orders! {
        #[tracing::instrument(skip_all)]
        #[resilient(courier, record_if = |_| !parcel.is_empty(), cancel = token)]
        async fn read_once_above, read_once_below(&self, parcel: String) -> Answer<String> {
            drop(token);
            depot(&parcel)?;
            Ok(parcel.clone())
        }
    }

    // Sent once under a predicate reading no argument: its one attempt
    // takes the argument it consumes, and its token.
    in_both_orders! {
        #[tracing::instrument(skip_all)]
        #[resilient(courier, record_if = |_| false, cancel = token)]
        async fn owned_above, owned_below(&self, parcel: String) -> Answer<String> {
            drop(token);
            depot(&parcel)?;
            Ok(parcel)
        }
    }
}

/// What the fallback below holds across its `.await`: past the 16 KiB
/// clippy's `large_futures` allows a future a developer awaits.
const LADEN: usize = 32 * 1024;

/// Written out, where a `macro_rules!` would make every `.await` its own:
/// the lint is denied on a method naming a fallback past its threshold,
/// under an instrument in either order, and on that method run again.
#[deny(clippy::large_futures)]
impl TracedService {
    #[tracing::instrument(skip_all)]
    #[resilient(courier, fallback = unladen)]
    async fn laden_above(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(courier, fallback = unladen)]
    #[tracing::instrument(skip_all)]
    async fn laden_below(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[tracing::instrument(skip_all)]
    #[resilient(courier, idempotent, fallback = unladen)]
    async fn laden_again_above(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(courier, idempotent, fallback = unladen)]
    #[tracing::instrument(skip_all)]
    async fn laden_again_below(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    async fn unladen(&self, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        let buffer = [7_u8; LADEN];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, retrying_once()))],
    providers = [TracedService],
)]
struct TracedModule;

/// What the instruments of this module filed while `Opened` is the default
/// subscriber: each span opened, by its name and the fields it declares,
/// and each event an `err` or a `ret` recorded, as its field and its value.
#[derive(Default)]
struct Filed {
    spans: Vec<(&'static str, Vec<&'static str>)>,
    events: Vec<(&'static str, String)>,
}

#[derive(Clone, Default)]
struct Opened(Arc<Mutex<Filed>>);

impl Opened {
    fn named(&self, name: &str) -> usize {
        self.fields(name).len()
    }

    /// The fields the spans named `name` declare, one list a span.
    fn fields(&self, name: &str) -> Vec<Vec<&'static str>> {
        let filed = self.0.lock().expect("unpoisoned");
        let named = filed.spans.iter().filter(|(opened, _)| *opened == name);
        named.map(|(_, fields)| fields.clone()).collect()
    }

    /// The events recorded since the last read, oldest first.
    fn recorded(&self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.0.lock().expect("unpoisoned").events)
    }
}

impl tracing::field::Visit for Opened {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let mut filed = self.0.lock().expect("unpoisoned");
        filed.events.push((field.name(), format!("{value:?}")));
    }
}

impl tracing::Subscriber for Opened {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let fields = span.metadata().fields().iter().map(|field| field.name());
        let mut filed = self.0.lock().expect("unpoisoned");
        filed.spans.push((span.metadata().name(), fields.collect()));
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    /// The policy files lines of its own, on its own target: only what an
    /// instrument of this module records is kept.
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == module_path!() {
            event.record(&mut self.clone());
        }
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

async fn traced() -> (HeadlessApp, Arc<TracedService>) {
    let app = boot::<TracedModule>().await;
    let traced = app
        .container()
        .get::<TracedService>()
        .expect("the module provides it");
    (app, traced)
}

const DOWN: &str = "the carrier did not answer";

#[tokio::test(start_paused = true)]
async fn a_method_instrumented_above_or_below_its_policy_runs_under_it() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let above = traced.above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    assert_eq!(traced.attempted(), 2, "sent again after its failure");
    assert_eq!(opened.named("above"), 2, "a span an attempt");

    let below = traced.below("P1".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
    assert_eq!(traced.attempted(), 2, "sent again after its failure");
    assert_eq!(opened.named("below"), 1, "one span for the whole run");

    let once = traced.above_once("P1".to_owned()).await;
    assert!(matches!(once.as_deref(), Ok("P1")), "{once:?}");
    let unknown = traced.above_once("P2".to_owned()).await;
    assert!(failed(&unknown), "{unknown:?}");
    assert_eq!(opened.named("above_once"), 2);
    assert_eq!(traced.attempted(), 0, "`above_once` counts no attempt");
    assert_eq!(opened.recorded(), [("error", DOWN.to_owned())]);

    let twice = traced.twice_above("P1".to_owned()).await;
    assert!(matches!(twice.as_deref(), Ok("P1")), "{twice:?}");
    assert_eq!(traced.attempted(), 2, "sent again after its failure");
    assert_eq!(opened.named("outer"), 2, "a span an attempt");
    assert_eq!(opened.named("inner"), 2, "a span an attempt");
    assert_eq!(opened.recorded(), [("error", DOWN.to_owned())]);
}

/// Above, `err` records what an attempt answered, the dependency's own
/// error; below, what the run answered, which here is no error.
#[tokio::test(start_paused = true)]
async fn an_err_above_records_each_attempts_error_and_below_the_runs() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let above = traced.err_above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("err_above"), 2, "a span an attempt");
    assert_eq!(opened.recorded(), [("error", DOWN.to_owned())]);

    let below = traced.err_below("P1".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("err_below"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [], "the run answered");

    // A `?` converts its error into the attempt's in both orders, and a
    // run ending on it is the one error the span below records.
    let above = traced.err_above("P2".to_owned()).await;
    assert!(failed(&above), "{above:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(
        opened.recorded(),
        [("error", DOWN.to_owned()), ("error", DOWN.to_owned())]
    );
    let below = traced.err_below("P2".to_owned()).await;
    assert!(failed(&below), "{below:?}");
    assert_eq!(traced.attempted(), 2);
    let run = below.expect_err("failed").to_string();
    assert_eq!(opened.recorded(), [("error", run)]);
}

#[tokio::test(start_paused = true)]
async fn a_ret_above_records_each_attempts_answer_and_below_the_runs() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let above = traced.ret_above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("ret_above"), 2, "a span an attempt");
    assert_eq!(
        opened.recorded(),
        [
            ("return", "Err(CarrierDown)".to_owned()),
            ("return", r#"Ok("P1")"#.to_owned()),
        ]
    );

    let below = traced.ret_below("P1".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("ret_below"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [("return", r#"Ok("P1")"#.to_owned())]);
}

#[tokio::test(start_paused = true)]
async fn an_err_and_a_ret_above_record_each_attempt_and_below_the_run() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let above = traced.both_above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("both_above"), 2, "a span an attempt");
    assert_eq!(
        opened.recorded(),
        [("error", DOWN.to_owned()), ("return", r#""P1""#.to_owned())]
    );

    let below = traced.both_below("P1".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("both_below"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [("return", r#""P1""#.to_owned())]);
}

/// The span's fields are read off the arguments as each attempt holds
/// them: skipped, recorded by name, or computed from one.
#[tokio::test(start_paused = true)]
async fn a_span_reads_the_arguments_it_records_at_each_attempt() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let above = traced.fields_above("P1".to_owned(), "north").await;
    assert!(matches!(above.as_deref(), Ok("P1 to north")), "{above:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(
        opened.fields("fields_above"),
        [["parcel", "weight"], ["parcel", "weight"]],
        "a span an attempt"
    );

    let below = traced.fields_below("P1".to_owned(), "north").await;
    assert!(matches!(below.as_deref(), Ok("P1 to north")), "{below:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(
        opened.fields("fields_below"),
        [["parcel", "weight"]],
        "one span for the whole run"
    );
}

#[tokio::test(start_paused = true)]
async fn a_method_run_once_consumes_its_argument_under_an_instrument_in_either_order() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let above = traced.once_above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    let unknown = traced.once_above("P2".to_owned()).await;
    assert!(failed(&unknown), "{unknown:?}");
    assert_eq!(traced.attempted(), 2, "each call sent once");
    assert_eq!(opened.named("once_above"), 2, "a span a call");
    assert_eq!(
        opened.recorded(),
        [("return", r#""P1""#.to_owned()), ("error", DOWN.to_owned())]
    );

    let below = traced.once_below("P1".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
    let unknown = traced.once_below("P2".to_owned()).await;
    assert!(failed(&unknown), "{unknown:?}");
    assert_eq!(traced.attempted(), 2, "each call sent once");
    assert_eq!(opened.named("once_below"), 2, "a span a call");
    let run = unknown.expect_err("failed").to_string();
    assert_eq!(
        opened.recorded(),
        [("return", r#""P1""#.to_owned()), ("error", run)]
    );
}

#[tokio::test(start_paused = true)]
async fn a_generic_method_and_one_borrowing_through_a_lifetime_run_in_either_order() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    assert!(matches!(traced.generic_above(7_u32).await, Ok(7)));
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("generic_above"), 2, "a span an attempt");
    assert!(matches!(
        traced.generic_below(("P1", 7_u8)).await,
        Ok(("P1", 7))
    ));
    assert_eq!(traced.attempted(), 2);
    assert_eq!(
        opened.named("generic_below"),
        1,
        "one span for the whole run"
    );

    let zone = "north".to_owned();
    assert!(matches!(traced.borrowed_above(&zone).await, Ok("north")));
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("borrowed_above"), 2, "a span an attempt");
    assert!(matches!(
        traced.borrowed_below("").await,
        Ok("the Lyon hub")
    ));
    assert_eq!(traced.attempted(), 2);
    assert_eq!(
        opened.named("borrowed_below"),
        1,
        "one span for the whole run"
    );
}

/// An instrumented attempt holds its span and its arguments across its
/// waits as the method's own future does: it runs on another task.
#[tokio::test(start_paused = true)]
async fn a_method_instrumented_above_its_policy_runs_on_a_task_of_its_own() {
    let (_app, traced) = traced().await;
    let sent = Arc::clone(&traced);
    let above = tokio::spawn(async move { sent.err_above("P1".to_owned()).await });
    let above = above.await.expect("no attempt panics");
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    let sent = Arc::clone(&traced);
    let once = tokio::spawn(async move { sent.once_above("P1".to_owned()).await });
    let once = once.await.expect("the attempt does not panic");
    assert!(matches!(once.as_deref(), Ok("P1")), "{once:?}");
}

/// A fallback's future past clippy's `large_futures` threshold answers
/// under an instrument in either order, the method run once or again.
#[tokio::test(start_paused = true)]
async fn a_fallback_holding_a_large_buffer_answers_under_an_instrument_in_either_order() {
    let (_app, traced) = traced().await;
    let above = traced.laden_above().await;
    assert!(matches!(above, Ok(LADEN)), "{above:?}");
    let below = traced.laden_below().await;
    assert!(matches!(below, Ok(LADEN)), "{below:?}");
    let above = traced.laden_again_above().await;
    assert!(matches!(above, Ok(LADEN)), "{above:?}");
    let below = traced.laden_again_below().await;
    assert!(matches!(below, Ok(LADEN)), "{below:?}");
}

/// Both orders hold under every other key. Above, each attempt opens a
/// span recording what that attempt answered, and the fallback runs
/// outside them all; below, one span covers the run and its fallback, and
/// records what the method answers.
#[tokio::test(start_paused = true)]
async fn a_method_carrying_every_other_key_runs_under_an_instrument_in_either_order() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let answered = traced.keyed_above("P1".to_owned()).await;
    assert!(matches!(answered.as_deref(), Ok("P1")), "{answered:?}");
    assert_eq!(traced.attempted(), 1);
    let above = traced.keyed_above("P2".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P2 held")), "{above:?}");
    assert_eq!(traced.attempted(), 2, "sent again, then its fallback");
    assert_eq!(opened.named("keyed_above"), 3, "a span an attempt");
    assert_eq!(
        opened.recorded(),
        [("error", DOWN.to_owned()), ("error", DOWN.to_owned())],
        "each attempt's own error"
    );

    let below = traced.keyed_below("P2".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P2 held")), "{below:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("keyed_below"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [], "the method answered its fallback's");

    let above = traced.lent_above("P2".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P2 held")), "{above:?}");
    assert_eq!(opened.named("lent_above"), 1);
    assert_eq!(
        opened.recorded(),
        [("return", "Err(CarrierDown)".to_owned())],
        "the attempt's answer, its fallback outside the span"
    );
    let below = traced.lent_below("P2".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P2 held")), "{below:?}");
    assert_eq!(opened.named("lent_below"), 1);
    assert_eq!(
        opened.recorded(),
        [("return", r#"Ok("P2 held")"#.to_owned())],
        "the method's answer, its fallback's"
    );

    let above = traced.owned_above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    let below = traced.owned_below("P2".to_owned()).await;
    assert!(failed(&below), "{below:?}");
    assert_eq!(
        (opened.named("owned_above"), opened.named("owned_below")),
        (1, 1)
    );
}

/// Both orders hold where a predicate reads an argument the method owns:
/// the future an instrument written above builds borrows it as the
/// attempt does, and the fallback takes it once the filter read it.
#[tokio::test(start_paused = true)]
async fn a_predicate_reads_an_owned_argument_under_an_instrument_in_either_order() {
    let (_app, traced) = traced().await;
    for (parcel, answers, sent) in [
        ("P1", Ok("P1"), 1),
        ("P2", Ok("P2 held"), 2),
        ("P0", Err(()), 1),
    ] {
        let above = traced.read_above(parcel.to_owned()).await;
        assert_eq!(above.as_deref().map_err(drop), answers, "above: {parcel}");
        assert_eq!(traced.attempted(), sent, "above: {parcel}");
        let below = traced.read_below(parcel.to_owned()).await;
        assert_eq!(below.as_deref().map_err(drop), answers, "below: {parcel}");
        assert_eq!(traced.attempted(), sent, "below: {parcel}");
    }

    let above = traced.read_once_above("P1".to_owned()).await;
    assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
    let below = traced.read_once_below("P2".to_owned()).await;
    assert!(failed(&below), "{below:?}");
    let sent = Arc::clone(&traced);
    let spawned = tokio::spawn(async move { sent.read_once_above("P1".to_owned()).await });
    let spawned = spawned.await.expect("the attempt does not panic");
    assert!(matches!(spawned.as_deref(), Ok("P1")), "{spawned:?}");
}
