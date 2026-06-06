//! `QueueWorker` configure fail-fast and `JobContext` wrapping; no Redis.
//!
//! Also covers the wire-format envelope: the `#[processor]` macro emits a
//! handler that unwraps `{ "v": <n>, "payload": <…> }` (current version),
//! accepts unversioned legacy payloads with a warning, and rejects unknown
//! versions with an `Err` (not a panic).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use nestrs_core::{Container, JobContext, Transport, injectable};
use nestrs_queue::{ProcessMethod, Processor, WIRE_FORMAT_VERSION, processor};
use nestrs_redis::QueueWorker;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

// A link-time `ProcessMethod` so `QueueWorker::configure` sees at least one
// processor in this test binary and exercises the missing-connection branch.
fn probe_handler(
    _payload: serde_json::Value,
    _container: Container,
) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>> {
    Box::pin(async { Ok(()) })
}

nestrs_core::inventory::submit! {
    ProcessMethod {
        name: "probe::process",
        queue: "test-queue",
        concurrency: 1,
        retries: 0,
        provider_type_id: || std::any::TypeId::of::<ProbeMarker>(),
        handler: probe_handler,
    }
}

struct ProbeMarker;

#[tokio::test]
async fn configure_fails_when_processors_exist_without_a_connection() {
    let container = Container::builder().build();

    let err = QueueWorker::new()
        .configure(&container)
        .await
        .expect_err("processors without QueueConnection abort configure");
    assert!(
        err.to_string().contains("QueueConnection"),
        "the error names the missing connection: {err}",
    );
}

#[tokio::test]
async fn configure_succeeds_with_no_processors_and_serve_idles_until_cancel() {
    // Mark our link-time probe entry unreachable so configure() sees zero
    // methods in this test (the access graph is the same filter the real
    // worker uses at boot).
    let container = Container::builder()
        .provide(nestrs_core::ReachableProviders(Default::default()))
        .build();
    let mut worker = QueueWorker::new();
    worker
        .configure(&container)
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

tokio::task_local! {
    static MARKER: u8;
}

static OBSERVED_MARKER: AtomicBool = AtomicBool::new(false);

struct MarkerContext;

impl JobContext for MarkerContext {
    fn scope<'a>(
        &'a self,
        inner: Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(MARKER.scope(7, inner))
    }
}

struct ProbeProcessor;

#[async_trait::async_trait]
impl nestrs_queue::Processor for ProbeProcessor {
    type Job = u8;

    async fn process(&self, _job: Self::Job) -> anyhow::Result<()> {
        if MARKER.try_with(|m| *m) == Ok(7) {
            OBSERVED_MARKER.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

impl nestrs_queue::FromContainer for ProbeProcessor {
    fn from_container(_container: &Container) -> Self {
        Self
    }
}

#[tokio::test]
async fn processors_run_inside_the_bound_job_context() {
    let container = Container::builder()
        .provide_dyn::<dyn JobContext>(Arc::new(MarkerContext))
        .build();

    let job_context = container
        .get_dyn::<dyn JobContext>()
        .expect("JobContext bound");
    nestrs_core::run_in_job_context(Some(&job_context), async {
        ProbeProcessor.process(1).await.expect("job succeeds");
    })
    .await;

    assert!(
        OBSERVED_MARKER.load(Ordering::SeqCst),
        "the processor ran inside the bound JobContext",
    );
}

// ---- Wire-format envelope (Bug 4) ------------------------------------------

const ENVELOPE_QUEUE: &str = "envelope-test";

static ENVELOPE_LAST_N: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Serialize, Deserialize)]
struct EnvelopeJob {
    n: u32,
}

#[injectable]
#[derive(Default)]
struct EnvelopeProc;

#[processor]
impl EnvelopeProc {
    #[process(queue = "envelope-test", concurrency = 1, retries = 0)]
    async fn handle(&self, job: EnvelopeJob) -> anyhow::Result<()> {
        ENVELOPE_LAST_N.store(job.n, Ordering::SeqCst);
        Ok(())
    }
}

fn envelope_handler() -> nestrs_queue::JobHandler {
    nestrs_core::inventory::iter::<ProcessMethod>()
        .find(|m| m.queue == ENVELOPE_QUEUE)
        .expect("the #[processor] above submits a ProcessMethod for envelope-test")
        .handler
}

fn envelope_container() -> Container {
    Container::builder().provide(EnvelopeProc).build()
}

#[tokio::test]
async fn v1_envelope_is_unwrapped_and_processed() {
    ENVELOPE_LAST_N.store(0, Ordering::SeqCst);
    let payload = json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "n": 42 },
    });
    envelope_handler()(payload, envelope_container())
        .await
        .expect("v=1 envelope drives the user method to Ok(())");
    assert_eq!(
        ENVELOPE_LAST_N.load(Ordering::SeqCst),
        42,
        "the user method saw the unwrapped payload",
    );
}

#[tokio::test]
async fn unversioned_legacy_payload_is_still_processed() {
    ENVELOPE_LAST_N.store(0, Ordering::SeqCst);
    // A raw payload — no `v` / `payload` wrapper — left in Redis from a prior
    // deploy must drain successfully (with a warn log) so a rolling deploy
    // doesn't drop jobs.
    let legacy = json!({ "n": 7 });
    envelope_handler()(legacy, envelope_container())
        .await
        .expect("legacy unversioned payload is decoded directly");
    assert_eq!(
        ENVELOPE_LAST_N.load(Ordering::SeqCst),
        7,
        "the user method saw the raw legacy payload",
    );
}

#[tokio::test]
async fn newer_wire_version_returns_err_pointing_at_the_producer() {
    ENVELOPE_LAST_N.store(0, Ordering::SeqCst);
    let from_the_future = json!({
        "v": (WIRE_FORMAT_VERSION as u64) + 99,
        "payload": { "n": 1 },
    });
    let err = envelope_handler()(from_the_future, envelope_container())
        .await
        .expect_err("an unknown wire-format version must surface as Err, not a panic");
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported job wire-format version"),
        "the error names the regression: {msg}",
    );
    // Direction-specific guidance — a newer producer means roll back the
    // consumer or wait for the producer to drain (not "drain the queue").
    assert!(
        msg.contains("newer release"),
        "newer-version error explains the producer is ahead: {msg}",
    );
    assert!(
        msg.contains("roll back this consumer") || msg.contains("wait for the producer"),
        "newer-version error tells the operator what to do: {msg}",
    );
    assert_eq!(
        ENVELOPE_LAST_N.load(Ordering::SeqCst),
        0,
        "the user method never ran",
    );
}

#[tokio::test]
async fn older_wire_version_returns_err_pointing_at_the_drain_path() {
    // A pinned-version producer (v=0) talking to a v=1 consumer must surface
    // the *opposite* guidance from the newer-version branch — drain the
    // queue or pin the consumer back. Bug 2C: pre-fix, the message said
    // "the producer is from a newer release" regardless of direction.
    ENVELOPE_LAST_N.store(0, Ordering::SeqCst);
    let from_the_past = json!({
        "v": 0u64,
        "payload": { "n": 1 },
    });
    let err = envelope_handler()(from_the_past, envelope_container())
        .await
        .expect_err("an older wire-format version must surface as Err, not a panic");
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported job wire-format version"),
        "the error names the regression: {msg}",
    );
    assert!(
        msg.contains("older release"),
        "older-version error names the producer direction: {msg}",
    );
    assert!(
        msg.contains("drain the queue") || msg.contains("pin the consumer"),
        "older-version error tells the operator what to do: {msg}",
    );
    assert_eq!(
        ENVELOPE_LAST_N.load(Ordering::SeqCst),
        0,
        "the user method never ran",
    );
}

#[tokio::test]
async fn missing_provider_returns_err_without_panicking() {
    // Bug 3: the macro used to `.expect()` a missing provider and crash the
    // apalis worker process. It must now surface an `Err` so apalis records
    // the failure, retries per budget, and the worker keeps draining other
    // queues.
    let container = Container::builder().build();
    let payload = json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "n": 1 },
    });
    let err = envelope_handler()(payload, container)
        .await
        .expect_err("a missing provider must surface as Err, not a panic");
    let msg = err.to_string();
    assert!(
        msg.contains("not registered"),
        "the error names the wiring defect: {msg}",
    );
}

#[tokio::test]
async fn payload_schema_drift_returns_err_without_panicking() {
    // Bug 3 sibling: a v=1 envelope whose payload doesn't match `EnvelopeJob`
    // must surface as Err so apalis applies the retry budget — not crash the
    // worker process via a panic.
    let payload = json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "wrong_field": "nope" },
    });
    let err = envelope_handler()(payload, envelope_container())
        .await
        .expect_err("a schema drift must surface as Err, not a panic");
    let msg = err.to_string();
    assert!(
        msg.contains("failed to deserialize job"),
        "the error names the decode failure: {msg}",
    );
}

// ---- Strict envelope detection (Bug X1 + X2) -------------------------------
//
// The envelope is `{ v: <int>, payload: <…> }` — exactly two keys, `v` a
// non-negative integer (a JSON Number, optionally float-valued like `1.0`).
// Anything else (extra keys, string `v`, negative `v`) is a *user job* that
// happens to share the field names — fall through to the legacy raw-decode
// path so the user method gets its data and no false-positive envelope error
// triggers.

const STRICT_QUEUE: &str = "strict-envelope-test";

static STRICT_LAST_V: AtomicU32 = AtomicU32::new(0);
static STRICT_LAST_PAYLOAD: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Serialize, Deserialize)]
struct StrictJob {
    // Same two field names as the wire envelope — this is the bug:
    // detection that only looks at "has v + has payload" mis-classifies
    // this user job as an envelope.
    v: u32,
    payload: u32,
    // A third field distinguishes a real user job from the wire envelope.
    id: u32,
}

#[injectable]
#[derive(Default)]
struct StrictProc;

#[processor]
impl StrictProc {
    #[process(queue = "strict-envelope-test", concurrency = 1, retries = 0)]
    async fn handle(&self, job: StrictJob) -> anyhow::Result<()> {
        STRICT_LAST_V.store(job.v, Ordering::SeqCst);
        STRICT_LAST_PAYLOAD.store(job.payload, Ordering::SeqCst);
        Ok(())
    }
}

fn strict_handler() -> nestrs_queue::JobHandler {
    nestrs_core::inventory::iter::<ProcessMethod>()
        .find(|m| m.queue == STRICT_QUEUE)
        .expect("the #[processor] above submits a ProcessMethod for strict-envelope-test")
        .handler
}

fn strict_container() -> Container {
    Container::builder().provide(StrictProc).build()
}

#[tokio::test]
async fn user_job_with_v_and_payload_keys_plus_a_third_is_not_an_envelope() {
    STRICT_LAST_V.store(0, Ordering::SeqCst);
    STRICT_LAST_PAYLOAD.store(0, Ordering::SeqCst);
    // Three keys (v, payload, id) — the third key disqualifies the object
    // from being an envelope. The legacy raw-decode path must drive the
    // user method, not the envelope branch.
    let user_job = json!({
        "v": 9,
        "payload": 100,
        "id": 42,
    });
    strict_handler()(user_job, strict_container())
        .await
        .expect("a user job with v+payload+id keys decodes as the user job");
    assert_eq!(STRICT_LAST_V.load(Ordering::SeqCst), 9);
    assert_eq!(STRICT_LAST_PAYLOAD.load(Ordering::SeqCst), 100);
}

#[tokio::test]
async fn float_valued_v_is_accepted_as_envelope_version() {
    ENVELOPE_LAST_N.store(0, Ordering::SeqCst);
    // A non-Rust producer may serialize `v` as `1.0` rather than `1` — the
    // envelope detection accepts an integer-valued float (current
    // WIRE_FORMAT_VERSION as f64). The user method then runs normally.
    let payload = json!({
        "v": WIRE_FORMAT_VERSION as f64,
        "payload": { "n": 21 },
    });
    envelope_handler()(payload, envelope_container())
        .await
        .expect("v=1.0 (float) envelope is unwrapped like v=1");
    assert_eq!(ENVELOPE_LAST_N.load(Ordering::SeqCst), 21);
}

#[tokio::test]
async fn string_v_falls_through_to_legacy_path() {
    // `"v": "1"` is not a JSON Number — outside the contract. Detection
    // must reject it as an envelope and fall through to legacy decode.
    // The legacy decode then fails for EnvelopeJob (because the top-level
    // shape is `{v, payload}` instead of `{n}`), surfacing as Err — not a
    // hard envelope-format error and not a panic.
    let payload = json!({
        "v": "1",
        "payload": { "n": 1 },
    });
    let err = envelope_handler()(payload, envelope_container())
        .await
        .expect_err("string `v` is not an envelope; legacy decode then fails for the wrong shape");
    let msg = err.to_string();
    assert!(
        msg.contains("failed to deserialize job"),
        "string-v falls through to legacy decode, which then surfaces a schema-drift error: {msg}",
    );
}

#[tokio::test]
async fn negative_v_falls_through_to_legacy_path() {
    // `"v": -1` is a JSON Number but not non-negative — outside the
    // contract. Detection must reject and fall through.
    let payload = json!({
        "v": -1,
        "payload": { "n": 1 },
    });
    let err = envelope_handler()(payload, envelope_container())
        .await
        .expect_err("negative `v` is not an envelope; legacy decode fails for the wrong shape");
    let msg = err.to_string();
    assert!(
        msg.contains("failed to deserialize job"),
        "negative-v falls through to legacy decode: {msg}",
    );
}

// ---- Panic survival (Bug X3) -----------------------------------------------
//
// A panic inside a `#[process]` method must surface as `Err` (caught by
// `CatchPanicLayer`) — never as an aborted worker. This test asserts the
// layer chain is wired by exercising the layered service directly: the
// chain converts the panic to `Error::Abort` so apalis treats it as a
// failed job and the worker keeps draining the queue.

#[tokio::test]
async fn catch_panic_layer_converts_a_panicking_handler_into_err() {
    use apalis::layers::catch_panic::CatchPanicLayer;
    use std::task::{Context, Poll};
    use tower::{Layer, Service};

    #[derive(Clone)]
    struct PanickingService;

    impl Service<apalis::prelude::Request<u8, ()>> for PanickingService {
        type Response = ();
        type Error = apalis::prelude::Error;
        type Future = std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Self::Response, Self::Error>>
                    + Send,
            >,
        >;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: apalis::prelude::Request<u8, ()>) -> Self::Future {
            Box::pin(async {
                // Simulate a user handler panicking (e.g. an `unwrap` on a
                // None value inside `#[process]`).
                panic!("simulated user-handler panic");
            })
        }
    }

    let layer = CatchPanicLayer::new();
    let mut service = layer.layer(PanickingService);
    let request = apalis::prelude::Request::new(0u8);
    let response = service.call(request).await;

    assert!(
        response.is_err(),
        "CatchPanicLayer must convert the panic into an apalis Error",
    );
    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("PanicError") && err_msg.contains("simulated user-handler panic"),
        "the error surfaces the panic message: {err_msg}",
    );
}
