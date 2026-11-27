//! `#[tracing::instrument]` written below `#[resilient]`, the one order the
//! decorator takes: its span covers the whole run — every attempt, the waits
//! between them, what the policy refuses unsent and the fallback — and its
//! `err` and `ret` record what the method answers. Each flavour of the
//! instrument that changes what it expands to is run, held to the attempts it
//! makes, the spans it opens and what each records. Written above, it is
//! refused (`resilient_below_an_instrument.stderr`).

use super::*;

#[injectable]
struct TracedService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    attempts: AtomicU32,
    hub: Hub,
}

impl TracedService {
    fn fails_first(&self) -> bool {
        self.attempts.fetch_add(1, Ordering::Relaxed) == 0
    }

    fn attempted(&self) -> u32 {
        self.attempts.swap(0, Ordering::Relaxed)
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(name = "below", skip_all)]
    async fn below(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip_all, err)]
    async fn err(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        depot(&parcel)?;
        Ok(parcel.clone())
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip_all, ret)]
    async fn ret(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip_all, err, ret)]
    async fn both(&self, parcel: String) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    // An owned argument the span records and each attempt reads again,
    // beside a field computed from one.
    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip(self, zone), fields(weight = parcel.len()))]
    async fn fields(&self, parcel: String, zone: &str) -> Answer<String> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(format!("{parcel} to {zone}"))
    }

    // Run once: the one attempt consumes the argument it was moved.
    #[resilient(courier)]
    #[tracing::instrument(skip_all, err, ret)]
    async fn once(&self, parcel: String) -> Answer<String> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        depot(&parcel)?;
        Ok(parcel)
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip_all, ret)]
    async fn generic<T>(&self, parcel: T) -> Answer<T>
    where
        T: std::fmt::Debug + Clone + Sync,
    {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(parcel.clone())
    }

    // The answer borrows its host or its argument through a lifetime the
    // method names.
    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip_all, err)]
    async fn borrowed<'a>(&'a self, zone: &'a str) -> Answer<&'a str> {
        if self.fails_first() {
            return Err(CarrierDown);
        }
        Ok(if zone.is_empty() { &self.hub.0 } else { zone })
    }

    /// The parcel held at the hub, in place of the error its run ended on.
    async fn held(&self, parcel: String, _error: ResilienceError<CarrierDown>) -> Answer<String> {
        Ok(format!("{parcel} held"))
    }

    // Every other key: each predicate, a fallback handed the argument
    // each attempt read, and the attempt's token read in the body.
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
    #[tracing::instrument(skip_all, err)]
    async fn keyed(&self, parcel: String) -> Answer<String> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        if token.is_cancelled() {
            return Ok(String::new());
        }
        depot(&parcel)?;
        Ok(parcel.clone())
    }

    // Sent once, its one attempt lending the argument its fallback takes.
    #[resilient(courier, fallback = held, cancel = token)]
    #[tracing::instrument(skip_all, ret)]
    async fn lent(&self, parcel: String) -> Answer<String> {
        let _attempt = token;
        depot(&parcel)?;
        Ok(parcel.clone())
    }

    // Every predicate reading the argument the method owns, which its
    // fallback is then handed by value.
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
    #[tracing::instrument(skip_all)]
    async fn read(&self, parcel: String) -> Answer<String> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        drop(token);
        depot(&parcel)?;
        Ok(parcel.clone())
    }

    // Sent once under a predicate reading the argument its one attempt
    // lends it.
    #[resilient(courier, record_if = |_| !parcel.is_empty(), cancel = token)]
    #[tracing::instrument(skip_all)]
    async fn read_once(&self, parcel: String) -> Answer<String> {
        drop(token);
        depot(&parcel)?;
        Ok(parcel.clone())
    }

    // Sent once under a predicate reading no argument: its one attempt
    // takes the argument it consumes, and its token.
    #[resilient(courier, record_if = |_| false, cancel = token)]
    #[tracing::instrument(skip_all)]
    async fn owned(&self, parcel: String) -> Answer<String> {
        drop(token);
        depot(&parcel)?;
        Ok(parcel)
    }
}

/// What the fallback below holds across its `.await`: past the 16 KiB
/// clippy's `large_futures` allows a future a developer awaits.
const LADEN: usize = 32 * 1024;

/// The lint is denied on a method naming a fallback past its threshold, run
/// once or again, under an instrument.
#[deny(clippy::large_futures)]
impl TracedService {
    #[resilient(courier, fallback = unladen)]
    #[tracing::instrument(skip_all)]
    async fn laden(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(courier, idempotent, fallback = unladen)]
    #[tracing::instrument(skip_all)]
    async fn laden_again(&self) -> Answer<usize> {
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

/// One span covers the whole run, its attempts and the wait between them.
#[tokio::test(start_paused = true)]
async fn a_method_instrumented_below_its_policy_runs_under_one_span() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let below = traced.below("P1".to_owned()).await;
    assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
    assert_eq!(traced.attempted(), 2, "sent again after its failure");
    assert_eq!(opened.named("below"), 1, "one span for the whole run");
}

/// `err` records what the run answered: nothing where an attempt failed and
/// the next answered, and the run's error, a `?` converting each attempt's,
/// where none did.
#[tokio::test(start_paused = true)]
async fn an_err_records_the_runs_error() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let answered = traced.err("P1".to_owned()).await;
    assert!(matches!(answered.as_deref(), Ok("P1")), "{answered:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("err"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [], "the run answered");

    let failing = traced.err("P2".to_owned()).await;
    assert!(failed(&failing), "{failing:?}");
    assert_eq!(traced.attempted(), 2);
    let run = failing.expect_err("failed").to_string();
    assert_eq!(opened.recorded(), [("error", run)]);
}

#[tokio::test(start_paused = true)]
async fn a_ret_records_the_runs_answer_and_an_err_beside_it_no_error() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let ret = traced.ret("P1".to_owned()).await;
    assert!(matches!(ret.as_deref(), Ok("P1")), "{ret:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("ret"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [("return", r#"Ok("P1")"#.to_owned())]);

    let both = traced.both("P1".to_owned()).await;
    assert!(matches!(both.as_deref(), Ok("P1")), "{both:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("both"), 1, "one span for the whole run");
    assert_eq!(opened.recorded(), [("return", r#""P1""#.to_owned())]);
}

/// The span's fields are read off the arguments as the method was called
/// with them: skipped, recorded by name, or computed from one.
#[tokio::test(start_paused = true)]
async fn a_span_reads_the_arguments_it_records_once_for_the_run() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let fields = traced.fields("P1".to_owned(), "north").await;
    assert!(matches!(fields.as_deref(), Ok("P1 to north")), "{fields:?}");
    assert_eq!(traced.attempted(), 2);
    assert_eq!(
        opened.fields("fields"),
        [["parcel", "weight"]],
        "one span for the whole run"
    );
}

#[tokio::test(start_paused = true)]
async fn a_method_run_once_consumes_its_argument_under_an_instrument() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let answered = traced.once("P1".to_owned()).await;
    assert!(matches!(answered.as_deref(), Ok("P1")), "{answered:?}");
    let unknown = traced.once("P2".to_owned()).await;
    assert!(failed(&unknown), "{unknown:?}");
    assert_eq!(traced.attempted(), 2, "each call sent once");
    assert_eq!(opened.named("once"), 2, "a span a call");
    let run = unknown.expect_err("failed").to_string();
    assert_eq!(
        opened.recorded(),
        [("return", r#""P1""#.to_owned()), ("error", run)]
    );
    let sent = Arc::clone(&traced);
    let spawned = tokio::spawn(async move { sent.once("P1".to_owned()).await });
    let spawned = spawned.await.expect("the attempt does not panic");
    assert!(matches!(spawned.as_deref(), Ok("P1")), "{spawned:?}");
}

#[tokio::test(start_paused = true)]
async fn a_generic_method_and_one_borrowing_through_a_lifetime_run_under_an_instrument() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    assert!(matches!(traced.generic(("P1", 7_u8)).await, Ok(("P1", 7))));
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("generic"), 1, "one span for the whole run");

    assert!(matches!(traced.borrowed("").await, Ok("the Lyon hub")));
    assert_eq!(traced.attempted(), 2);
    let zone = "north".to_owned();
    assert!(matches!(traced.borrowed(&zone).await, Ok("north")));
    assert_eq!(traced.attempted(), 2);
    assert_eq!(opened.named("borrowed"), 2, "a span a call");
}

/// A fallback's future past clippy's `large_futures` threshold answers under
/// an instrument, the method run once or again.
#[tokio::test(start_paused = true)]
async fn a_fallback_holding_a_large_buffer_answers_under_an_instrument() {
    let (_app, traced) = traced().await;
    let once = traced.laden().await;
    assert!(matches!(once, Ok(LADEN)), "{once:?}");
    let again = traced.laden_again().await;
    assert!(matches!(again, Ok(LADEN)), "{again:?}");
}

/// Under every other key, one span covers the run and its fallback, and
/// records what the method answers.
#[tokio::test(start_paused = true)]
async fn a_method_carrying_every_other_key_runs_under_an_instrument() {
    let (_app, traced) = traced().await;
    let opened = Opened::default();
    let _default = tracing::subscriber::set_default(opened.clone());

    let answered = traced.keyed("P1".to_owned()).await;
    assert!(matches!(answered.as_deref(), Ok("P1")), "{answered:?}");
    assert_eq!(traced.attempted(), 1);
    let held = traced.keyed("P2".to_owned()).await;
    assert!(matches!(held.as_deref(), Ok("P2 held")), "{held:?}");
    assert_eq!(traced.attempted(), 2, "sent again, then its fallback");
    assert_eq!(opened.named("keyed"), 2, "a span a call");
    assert_eq!(opened.recorded(), [], "the method answered its fallback's");

    let lent = traced.lent("P2".to_owned()).await;
    assert!(matches!(lent.as_deref(), Ok("P2 held")), "{lent:?}");
    assert_eq!(opened.named("lent"), 1);
    assert_eq!(
        opened.recorded(),
        [("return", r#"Ok("P2 held")"#.to_owned())],
        "the method's answer, its fallback's"
    );

    let owned = traced.owned("P1".to_owned()).await;
    assert!(matches!(owned.as_deref(), Ok("P1")), "{owned:?}");
    let owned = traced.owned("P2".to_owned()).await;
    assert!(failed(&owned), "{owned:?}");
    assert_eq!(opened.named("owned"), 2);
}

/// A predicate reads an argument the method owns under an instrument: the
/// fallback takes it once the filter read it, and the method run once lending
/// it runs on a task of its own.
#[tokio::test(start_paused = true)]
async fn a_predicate_reads_an_owned_argument_under_an_instrument() {
    let (_app, traced) = traced().await;
    for (parcel, answers, sent) in [
        ("P1", Ok("P1"), 1),
        ("P2", Ok("P2 held"), 2),
        ("P0", Err(()), 1),
    ] {
        let read = traced.read(parcel.to_owned()).await;
        assert_eq!(read.as_deref().map_err(drop), answers, "{parcel}");
        assert_eq!(traced.attempted(), sent, "{parcel}");
    }

    let unknown = traced.read_once("P2".to_owned()).await;
    assert!(failed(&unknown), "{unknown:?}");
    let sent = Arc::clone(&traced);
    let spawned = tokio::spawn(async move { sent.read_once("P1".to_owned()).await });
    let spawned = spawned.await.expect("the attempt does not panic");
    assert!(matches!(spawned.as_deref(), Ok("P1")), "{spawned:?}");
}
