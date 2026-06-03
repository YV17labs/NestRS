//! `QueueWorker` configure fail-fast and `JobContext` wrapping; no Redis.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nestrs_core::{Container, JobContext, Transport};
use nestrs_queue::{Processor, ProcessorMeta, QueueWorker};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn configure_fails_when_processors_exist_without_a_connection() {
    struct ProbeHost;

    let container = Container::builder()
        .attach_meta::<ProbeHost, ProcessorMeta>(ProcessorMeta {
            name: "probe",
            queue: "test-queue",
            concurrency: 1,
            retries: 0,
            register: nestrs_queue::register_worker::<ProbeProcessor>,
        })
        .build();

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
    let container = Container::builder().build();
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

    let job_context = container.get_dyn::<dyn JobContext>().expect("JobContext bound");
    nestrs_core::run_in_job_context(Some(&job_context), async {
        ProbeProcessor.process(1).await.expect("job succeeds");
    })
    .await;

    assert!(
        OBSERVED_MARKER.load(Ordering::SeqCst),
        "the processor ran inside the bound JobContext",
    );
}
