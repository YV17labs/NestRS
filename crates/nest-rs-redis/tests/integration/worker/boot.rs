//! `RedisWorker`'s boot, in process and without Redis: what `configure` refuses
//! before the queue storage is ever opened — methods with no connection, two
//! methods on one queue — what it lets through, and the idle serve of an app
//! with nothing to drain.
//!
//! What an attempt at a job *is* — the envelope, the budget, the outcome classes
//! — belongs to the port and is proved by its own suite; what only a live worker
//! shows is in `e2e`.

use std::any::TypeId;
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::time::Duration;

use nest_rs_core::{Container, ReachableProviders, Transport};
use nest_rs_queue::{HandlerContext, JobError, ProcessMethod, ProcessOptions, Throttle};
use nest_rs_redis::{RedisWorker, RedisWorkerConfig};
use tokio_util::sync::CancellationToken;

type Handled = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

fn never_runs(_payload: serde_json::Value, _context: HandlerContext) -> Handled {
    Box::pin(async { Ok(()) })
}

struct ProbeHost;
struct FirstClaimant;
struct SecondClaimant;
struct ThrottledHost;
struct CheckpointHost;

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "ProbeHost::run", "probe",
        ProcessOptions::DEFAULT,
        TypeId::of::<ProbeHost>, never_runs,
    )
}

// Two entries draining one queue — the shape a backend used to accept, building
// one apalis worker per entry so both polled the same stream.
nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "FirstClaimant::drain", "contested",
        ProcessOptions::DEFAULT.with_retries(1),
        TypeId::of::<FirstClaimant>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "SecondClaimant::drain", "contested",
        ProcessOptions::DEFAULT.with_retries(9),
        TypeId::of::<SecondClaimant>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "ThrottledHost::run", "throttled",
        ProcessOptions::DEFAULT.with_throttle(Throttle::new(NonZeroU32::MIN, Duration::from_secs(60))),
        TypeId::of::<ThrottledHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "CheckpointHost::run", "resumable",
        ProcessOptions::DEFAULT.with_checkpoint(true),
        TypeId::of::<CheckpointHost>, never_runs,
    )
}

/// A container reaching exactly `providers` — the access graph's filter, which
/// decides which of the `ProcessMethod`s linked into this binary a worker sees.
fn reaching(providers: &[TypeId]) -> Container {
    Container::builder()
        .provide(ReachableProviders(providers.iter().copied().collect()))
        .build()
}

/// What `configure` answers for an app reaching exactly what `container` seeds.
async fn configure(container: &Container) -> anyhow::Result<()> {
    RedisWorker::new().configure(container).await
}

#[tokio::test]
async fn configure_fails_when_processors_exist_without_a_connection() {
    // Name the reachable provider rather than leaving the set unseeded: with no
    // gating, every `ProcessMethod` linked into this binary is visible, so a
    // later entry could route this boot into a different refusal.
    let refusal = configure(&reaching(&[TypeId::of::<ProbeHost>()]))
        .await
        .expect_err("processors without RedisConnection abort configure")
        .to_string();
    assert!(
        refusal.contains("RedisConnection"),
        "the error names the missing connection: {refusal}",
    );
}

#[tokio::test]
async fn two_processors_claiming_one_queue_fail_configure() {
    let refusal = configure(&reaching(&[
        TypeId::of::<FirstClaimant>(),
        TypeId::of::<SecondClaimant>(),
    ]))
    .await
    .expect_err("two claimants on one queue abort configure")
    .to_string();
    for part in ["contested", "FirstClaimant::drain", "SecondClaimant::drain"] {
        assert!(
            refusal.contains(part),
            "the refusal names {part}: {refusal}"
        );
    }
}

#[tokio::test]
async fn a_processor_another_app_owns_does_not_contest_this_queue() {
    // Discovery refuses *after* module-gating, so a second claimant linked into
    // the binary but outside this app's module tree is not this app's problem —
    // the whole point of per-app subsets.
    let refusal = configure(&reaching(&[TypeId::of::<FirstClaimant>()]))
        .await
        .expect_err("one claimant still needs a connection")
        .to_string();
    assert!(
        refusal.contains("RedisConnection"),
        "it got past the duplicate check to the connection check: {refusal}",
    );
}

/// Every declaration a `#[process]` can make is one this backend honours, so a
/// method declaring a throttle or a checkpoint gets past discovery — as far as
/// the connection, which this suite never opens. The refusal a backend without
/// them owes is the port's, proved in its own suite.
#[tokio::test]
async fn a_throttle_or_a_checkpoint_gets_past_discovery_on_this_backend() {
    for host in [
        TypeId::of::<ThrottledHost>(),
        TypeId::of::<CheckpointHost>(),
    ] {
        let refusal = configure(&reaching(&[host]))
            .await
            .expect_err("no connection is seeded")
            .to_string();
        assert!(
            refusal.contains("RedisConnection") && !refusal.contains("does not provide"),
            "discovery served the method, and only the connection was missing: {refusal}",
        );
    }
}

#[tokio::test]
async fn configure_succeeds_with_no_processors_and_serve_idles_until_cancel() {
    // Nothing reachable, so configure() sees zero methods — the access graph is
    // the same filter the real worker uses at boot.
    let mut worker = RedisWorker::new();
    worker
        .configure(&reaching(&[]))
        .await
        .expect("an empty worker configures");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(worker).serve(cancel.clone()));
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");
}

/// The worker's stop is the drain window the deployment configured, read at
/// `configure` — the bound the boot line adds to the way down — and the default
/// window before it.
#[tokio::test]
async fn the_stop_bound_is_the_configured_drain_window() {
    let mut worker = RedisWorker::new();
    assert_eq!(
        worker.stop_bound(),
        RedisWorkerConfig::default().shutdown_timeout
    );
    let window = Duration::from_secs(7);
    let container = Container::builder()
        .provide(ReachableProviders(Default::default()))
        .provide(RedisWorkerConfig {
            shutdown_timeout: window,
            ..RedisWorkerConfig::default()
        })
        .build();
    worker
        .configure(&container)
        .await
        .expect("an empty worker configures");
    assert_eq!(worker.stop_bound(), window);
}

// ---- Panic backstop ----------------------------------------------------------
//
// The port catches a handler's panic inside the attempt, so this layer sees none
// of those. It stays for a panic outside that call — apalis's own fetch and
// decode path, the closure prologue — and it has to turn one into apalis's
// `Abort`, which dead-letters, rather than let it unwind the worker. This drives
// the layer the worker wires, over a service that panics.

#[tokio::test]
async fn the_panic_backstop_turns_a_panic_into_an_apalis_abort() {
    use apalis::layers::catch_panic::CatchPanicLayer;
    use std::task::{Context, Poll};
    use tower::{Layer, Service};

    #[derive(Clone)]
    struct PanickingService;

    impl Service<apalis::prelude::Request<u8, ()>> for PanickingService {
        type Response = ();
        type Error = apalis::prelude::Error;
        type Future = std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
        >;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: apalis::prelude::Request<u8, ()>) -> Self::Future {
            Box::pin(async {
                panic!("simulated panic outside the attempt");
            })
        }
    }

    let mut service = CatchPanicLayer::new().layer(PanickingService);
    let response = service.call(apalis::prelude::Request::new(0u8)).await;

    let Err(error) = response else {
        panic!("the layer must convert the panic into an apalis error");
    };
    assert!(
        matches!(error, apalis::prelude::Error::Abort(_)),
        "an Abort, which apalis's acknowledgement kills rather than re-queues: {error}",
    );
    assert!(
        error
            .to_string()
            .contains("simulated panic outside the attempt"),
        "the error carries the panic message: {error}",
    );
}
