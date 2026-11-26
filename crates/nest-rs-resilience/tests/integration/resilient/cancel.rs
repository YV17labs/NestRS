//! What `cancel` binds in a body: its attempt's own token, cancelled when the
//! policy gives that attempt up, under whichever shape the attempt takes its
//! arguments in — owning them, or lending them to the next attempt or to a
//! fallback.

use super::*;
use nest_rs_resilience::CancellationToken;

#[injectable]
struct DispatchService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    attempts: AtomicU32,
    /// Each attempt's token, as its body read it.
    tokens: Mutex<Vec<CancellationToken>>,
    /// The work an attempt handed to another task.
    work: Mutex<Option<tokio::task::JoinHandle<&'static str>>>,
}

impl DispatchService {
    fn keep(&self, token: &CancellationToken) {
        self.tokens.lock().expect("unpoisoned").push(token.clone());
    }

    /// Whether each token a body read is cancelled, in the order read.
    fn cancelled(&self) -> Vec<bool> {
        let tokens = std::mem::take(&mut *self.tokens.lock().expect("unpoisoned"));
        tokens.iter().map(CancellationToken::is_cancelled).collect()
    }

    /// Hands its work to another task, which owns the token, and never
    /// answers.
    #[resilient(courier, timeout = "100ms", cancel = token)]
    async fn dispatch(&self) -> Answer<()> {
        let work = tokio::spawn(async move {
            token.cancelled().await;
            "stopped"
        });
        *self.work.lock().expect("unpoisoned") = Some(work);
        std::future::pending().await
    }

    /// Its first attempt never answers; its second hands its token on.
    #[resilient(courier, idempotent, timeout = "100ms", cancel = token)]
    async fn redispatch(&self, parcel: &str) -> Answer<String> {
        self.keep(&token);
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            std::future::pending::<()>().await;
        }
        let live = !token.is_cancelled();
        let work = tokio::spawn(async move {
            token.cancelled().await;
            "stopped"
        });
        *self.work.lock().expect("unpoisoned") = Some(work);
        Ok(format!("{parcel} dispatched while live: {live}"))
    }

    async fn parked(&self, parcel: &str, _error: ResilienceError<CarrierDown>) -> Answer<String> {
        Ok(format!("{parcel} parked"))
    }

    #[resilient(courier, fallback = parked, cancel = attempt)]
    async fn park(&self, parcel: &str) -> Answer<String> {
        self.keep(&attempt);
        if parcel.is_empty() {
            return Ok(String::new());
        }
        Err(CarrierDown)
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, retrying_once()))],
    providers = [DispatchService],
)]
struct DispatchModule;

async fn dispatches() -> (HeadlessApp, Arc<DispatchService>) {
    let app = boot::<DispatchModule>().await;
    let dispatches = app
        .container()
        .get::<DispatchService>()
        .expect("the module provides it");
    (app, dispatches)
}

#[tokio::test(start_paused = true)]
async fn the_token_a_body_reads_is_cancelled_as_its_attempt_is_given_up_and_stops_work_handed_on() {
    let (_app, dispatches) = dispatches().await;

    let given_up = dispatches.dispatch().await;
    assert!(
        matches!(given_up, Err(ResilienceError::TimedOut { .. })),
        "{given_up:?}"
    );
    let work = dispatches.work.lock().expect("unpoisoned").take();
    let stopped = tokio::time::timeout(Duration::from_secs(1), work.expect("handed on"))
        .await
        .expect("the task the body spawned is told its attempt was given up");
    assert_eq!(stopped.ok(), Some("stopped"));
}

#[tokio::test(start_paused = true)]
async fn each_attempt_reads_a_token_of_its_own() {
    let (_app, dispatches) = dispatches().await;

    let answered = dispatches.redispatch("P1").await;
    assert!(
        matches!(answered.as_deref(), Ok("P1 dispatched while live: true")),
        "{answered:?}"
    );
    assert_eq!(
        dispatches.cancelled(),
        [true, false],
        "the attempt given up is cancelled, the one that answered lives on"
    );
    let work = dispatches.work.lock().expect("unpoisoned").take();
    assert!(
        !work.expect("handed on").is_finished(),
        "work the answering attempt handed on is not stopped by its answer"
    );
}

#[tokio::test(start_paused = true)]
async fn an_attempt_lending_its_arguments_to_a_fallback_reads_its_token_too() {
    let (_app, dispatches) = dispatches().await;

    let parked = dispatches.park("P1").await;
    assert!(matches!(parked.as_deref(), Ok("P1 parked")), "{parked:?}");
    assert_eq!(
        dispatches.cancelled(),
        [false],
        "an attempt that answered was not given up"
    );
}
